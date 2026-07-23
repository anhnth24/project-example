#!/usr/bin/env python3
"""O03 blue/green backup + restore-green pipeline (promote/cutover disabled)."""

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import time
import uuid
from pathlib import Path
from typing import Any
from urllib.parse import urlparse
from urllib.request import Request, urlopen

_LIB = Path(__file__).resolve().parent
if str(_LIB) not in sys.path:
    sys.path.insert(0, str(_LIB))

from manifest import (  # noqa: E402
    ManifestError,
    SCHEMA_VERSION,
    load_authenticated_manifest,
    require_signing_env,
    verify_artifacts,
    write_signed_manifest,
)
from pg_identity import (  # noqa: E402
    PgIdentityError,
    read_identity,
)
from pg_session import (  # noqa: E402
    PgSession,
    PgSessionError,
    assert_no_mc_credentials_argv,
    assert_no_password_argv,
    private_pg_env,
)
from targets import (  # noqa: E402
    CreationToken,
    GreenAllowlists,
    TargetError,
    assert_not_blue_alias,
)

ROOT = Path(__file__).resolve().parents[3]
ADVISORY_LOCK_KEY = 7303003


class PipelineError(RuntimeError):
    pass


def _umask_secure() -> int:
    return os.umask(0o077)


def assert_encryption_policy(dest: Path) -> None:
    """Require encryption or an explicit unencrypted-destination policy."""
    if os.environ.get("MARKHAND_BACKUP_ENCRYPTED") == "1":
        marker = dest.parent / ".markhand-backup-encrypted"
        if not marker.is_file():
            raise PipelineError(
                "MARKHAND_BACKUP_ENCRYPTED=1 but encrypted destination marker missing"
            )
        return
    policy = os.environ.get("MARKHAND_BACKUP_UNENCRYPTED_DEST_POLICY", "")
    if policy == "explicit_poc_tmp_only":
        resolved = dest.resolve()
        if "tmp" not in resolved.parts:
            raise PipelineError(
                "unencrypted dest policy explicit_poc_tmp_only requires path under tmp/"
            )
        return
    raise PipelineError(
        "backup encryption required (set MARKHAND_BACKUP_ENCRYPTED=1 + marker, "
        "or MARKHAND_BACKUP_UNENCRYPTED_DEST_POLICY=explicit_poc_tmp_only)"
    )


def app_mutation_write_gate_sufficient() -> bool:
    """True only when the central write-gate architecture contract is present.

    Requires middleware + advisory lock 7303003 + router wiring + background
    skip hooks. A stray `ops_fence::` import is intentionally insufficient.
    """
    from write_gate_contract import app_mutation_write_gate_sufficient_in

    server_src = ROOT / "crates" / "server" / "src"
    if not server_src.is_dir():
        return False
    return app_mutation_write_gate_sufficient_in(server_src)


def assert_consistency_write_gate() -> str:
    """Return watermark writeGate label, or raise if consistency required & absent."""
    require = os.environ.get("MARKHAND_BACKUP_REQUIRE_APP_WRITE_GATE", "1") == "1"
    if app_mutation_write_gate_sufficient():
        return "app_mutation_write_gate+ops_fences.restore"
    if require:
        raise PipelineError(
            "REFUSING_CONSISTENCY_BACKUP_WRITE_GATE_UNAVAILABLE: "
            "app mutation routes do not consult ops_fence; "
            "set MARKHAND_BACKUP_REQUIRE_APP_WRITE_GATE=0 only for "
            "non-consistency isolation drills (records gap watermark)"
        )
    return "fence_drain_lock_app_write_gate_absent"


def run(
    cmd: list[str],
    *,
    env: dict[str, str] | None = None,
    input_bytes: bytes | None = None,
) -> bytes:
    assert_no_password_argv(cmd)
    assert_no_mc_credentials_argv(cmd)
    proc = subprocess.run(
        cmd,
        check=False,
        capture_output=True,
        input=input_bytes,
        env=env or os.environ.copy(),
    )
    if proc.returncode != 0:
        err = (proc.stderr or b"").decode(errors="replace")[:300]
        raise PipelineError(f"command failed: {cmd[0]} rc={proc.returncode} err={err!r}")
    return proc.stdout


def psql(url: str, sql: str) -> str:
    with private_pg_env(url) as (safe_url, env):
        out = run(["psql", safe_url, "-v", "ON_ERROR_STOP=1", "-Atc", sql], env=env)
    return out.decode().strip()


def maintenance_url(database_url: str) -> str:
    parsed = urlparse(database_url)
    return parsed._replace(path="/postgres").geturl()


def db_name_from_url(database_url: str) -> str:
    name = (urlparse(database_url).path or "/").lstrip("/")
    if not name or not re.fullmatch(r"[A-Za-z0-9_]+", name):
        raise PipelineError("green database name unsafe/missing")
    return name


def _psql_vars(url: str, sql: str, variables: dict[str, str]) -> str:
    """Run SQL with validated bind values via set_config (psql -c ignores :'vars')."""
    preamble: list[str] = []
    for key, value in variables.items():
        if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", key):
            raise PipelineError("invalid psql variable name")
        if not re.fullmatch(r"[A-Za-z0-9._:/-]+", value or ""):
            raise PipelineError(f"unsafe psql bind value for {key}")
        preamble.append(f"SELECT set_config('markhand.{key}', '{value}', true);")
    rewritten = sql
    for key in variables:
        rewritten = rewritten.replace(f":'{key}'", f"current_setting('markhand.{key}')")
    script = "\n".join(preamble + [rewritten])
    out = psql(url, script)
    lines = [ln for ln in out.splitlines() if ln.strip()]
    return lines[-1] if lines else ""


def parse_returning_timestamptz(raw: str) -> str:
    """Extract a timestamptz token from psql RETURNING output."""
    text = (raw or "").strip()
    if not text:
        raise PipelineError("RETURNING timestamp empty")
    # Last non-empty line; accept ISO-ish timestamptz.
    line = [ln for ln in text.splitlines() if ln.strip()][-1].strip()
    if not re.search(r"\d{4}-\d{2}-\d{2}", line):
        raise PipelineError(f"RETURNING timestamp unparseable: {line!r}")
    return line


def drain_jobs_strict(session: PgSession, timeout_s: int = 30) -> None:
    """Wait for in-flight jobs; fail closed without mutating job rows."""
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        n = session.query(
            "SELECT count(*) FROM jobs WHERE status IN ('pending','leased','running');"
        )
        if n == "0":
            return
        time.sleep(0.5)
    # Never cancel / UPDATE jobs — strict fail only.
    left = session.query(
        "SELECT count(*) FROM jobs WHERE status IN ('pending','leased','running');"
    )
    raise PipelineError(
        f"STRICT_DRAIN_FAILED: {left} in-flight jobs remain; no job mutation performed"
    )


def set_fence(session: PgSession, stamp: str, epoch: str) -> str:
    if not re.fullmatch(r"[A-Za-z0-9._:/-]+", stamp):
        raise PipelineError("unsafe stamp")
    if not re.fullmatch(
        r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}", epoch
    ):
        raise PipelineError("unsafe epoch")
    raw = session.query(
        f"""
        INSERT INTO ops_fences (name, reason, active, set_at, cleared_at, set_by, attestation_sha256)
        VALUES (
          'restore',
          'backup capture {stamp} fence_epoch={epoch}',
          true, now(), NULL, 'backup.sh', NULL
        )
        ON CONFLICT (name) DO UPDATE
        SET reason = EXCLUDED.reason, active = true, set_at = now(), cleared_at = NULL,
            set_by = EXCLUDED.set_by, attestation_sha256 = NULL
        RETURNING set_at;
        """
    )
    return parse_returning_timestamptz(raw)


def mc_env_for(endpoint: str) -> dict[str, str]:
    parsed = urlparse(endpoint)
    host = parsed.hostname or "127.0.0.1"
    port = parsed.port or (443 if parsed.scheme == "https" else 80)
    env = os.environ.copy()
    env["MC_HOST_markhand"] = (
        f"{parsed.scheme}://{os.environ['MINIO_ACCESS_KEY']}:"
        f"{os.environ['MINIO_SECRET_KEY']}@{host}:{port}"
    )
    return env


def minio_inventory_events(inv_jsonl: bytes) -> dict[str, list[dict[str, Any]]]:
    """Chronological per-key MinIO version/delete-marker events."""
    by_key: dict[str, list[dict[str, Any]]] = {}
    for line in inv_jsonl.decode("utf-8", errors="replace").splitlines():
        if not line.strip():
            continue
        row = json.loads(line)
        key = row.get("key") or row.get("Key")
        if not key:
            continue
        by_key.setdefault(str(key), []).append(row)

    def sort_key(r: dict[str, Any]) -> tuple[str, str]:
        return (
            str(r.get("lastModified") or r.get("last_modified") or ""),
            str(r.get("versionId") or r.get("version_id") or ""),
        )

    return {k: sorted(rows, key=sort_key) for k, rows in sorted(by_key.items())}


def build_normalized_history(
    events: dict[str, list[dict[str, Any]]],
    *,
    content_by_version: dict[tuple[str, str], tuple[int, str]],
) -> list[dict[str, Any]]:
    """Ordered per-key history: event type, size, content hash (no versionId/timestamps)."""
    out: list[dict[str, Any]] = []
    for key, rows in events.items():
        hist: list[dict[str, Any]] = []
        for row in rows:
            version = str(row.get("versionId") or row.get("version_id") or "null")
            is_delete = bool(row.get("deleteMarker") or row.get("isDeleteMarker"))
            if is_delete:
                hist.append({"type": "delete", "size": None, "contentSha256": None})
                continue
            meta = content_by_version.get((key, version))
            if meta is None:
                raise PipelineError(
                    f"missing intermediate MinIO put content for {key} version={version}"
                )
            size, digest = meta
            hist.append({"type": "put", "size": size, "contentSha256": digest})
        out.append({"key": key, "events": hist})
    return out


def compare_normalized_history(
    expected: list[dict[str, Any]], actual: list[dict[str, Any]]
) -> None:
    exp = {e["key"]: e["events"] for e in expected}
    act = {e["key"]: e["events"] for e in actual}
    if set(exp) != set(act):
        raise PipelineError(
            f"normalized MinIO key set mismatch exp={sorted(exp)} act={sorted(act)}"
        )
    for key in sorted(exp):
        if exp[key] != act[key]:
            raise PipelineError(
                f"normalized MinIO history mismatch for {key}: "
                f"missing intermediate or type/size/hash differ "
                f"exp={exp[key]!r} act={act[key]!r}"
            )


def capture(dest: Path) -> Path:
    old_umask = _umask_secure()
    try:
        require_signing_env()
        assert_encryption_policy(dest)
        write_gate_label = assert_consistency_write_gate()

        url = os.environ["DATABASE_URL"]
        bucket = os.environ["MINIO_BUCKET"]
        endpoint = os.environ["MINIO_ENDPOINT"]
        qurl = os.environ["QDRANT_URL"].rstrip("/")
        collection = os.environ.get("QDRANT_COLLECTION", "markhand")
        stamp = os.environ.get("MARKHAND_BACKUP_STAMP") or time.strftime(
            "%Y%m%dT%H%M%SZ", time.gmtime()
        )
        key_id = os.environ["MARKHAND_BACKUP_KEY_ID"]

        # No DDL — ops_fences must already exist from reviewed migrations.
        fence_tbl = psql(
            url,
            "SELECT to_regclass('public.ops_fences') IS NOT NULL;",
        )
        if fence_tbl not in {"t", "true"}:
            raise PipelineError("ops_fences table absent; refuse backup (no DDL)")

        work = dest.parent / f".inprogress-{stamp}-{uuid.uuid4().hex[:8]}"
        if work.exists():
            shutil.rmtree(work)
        work.mkdir(parents=True, mode=0o700)
        (work / "objects").mkdir(mode=0o700)

        with PgSession(url) as session:
            if not session.try_advisory_lock(ADVISORY_LOCK_KEY):
                raise PipelineError("exclusive backup advisory lock unavailable")
            try:
                epoch = str(uuid.uuid4())
                fence_set_at = set_fence(session, stamp, epoch)
                drain_jobs_strict(session)
                lsn = session.query("SELECT pg_current_wal_lsn()::text;")
                identity = {
                    "pgSystemIdentifier": session.query(
                        "SELECT system_identifier::text FROM pg_control_system();"
                    ),
                    "pgDatabase": session.query("SELECT current_database();"),
                }
                if not identity["pgSystemIdentifier"].isdigit():
                    raise PipelineError("invalid pg system identifier")

                capture_start = int(time.time())
                (work / "capture-start.epoch").write_text(
                    f"{capture_start}\n", encoding="utf-8"
                )
                (work / "fence-epoch.txt").write_text(epoch + "\n", encoding="utf-8")
                (work / "WRITE_FENCE").write_text(
                    f"active blue/green fence at {stamp}\nfenceEpoch={epoch}\n",
                    encoding="utf-8",
                )

                dump = work / "postgres.dump"
                with private_pg_env(url) as (safe_url, dump_env):
                    run(
                        [
                            "pg_dump",
                            "--no-owner",
                            "--format=custom",
                            f"--file={dump}",
                            safe_url,
                        ],
                        env=dump_env,
                    )
                if dump.stat().st_size == 0:
                    raise PipelineError("postgres.dump empty")

                migrations = session.query(
                    "SELECT coalesce(string_agg(name, ',' ORDER BY name), '') "
                    "FROM markhand_schema_migrations;"
                ).split(",")
                migrations = [m for m in migrations if m]
                if not migrations:
                    raise PipelineError("markhand_schema_migrations empty")

                env_mc = mc_env_for(endpoint)
                ver = run(["mc", "version", "info", f"markhand/{bucket}"], env=env_mc).decode()
                if "Enabled" not in ver and "enabled" not in ver.lower():
                    raise PipelineError("MinIO bucket versioning must be Enabled")
                inv = work / "minio-versions.jsonl"
                inv_bytes = run(
                    ["mc", "ls", "--recursive", "--json", "--versions", f"markhand/{bucket}"],
                    env=env_mc,
                )
                inv.write_bytes(inv_bytes)
                (work / "minio-versions.txt").write_bytes(
                    run(
                        ["mc", "ls", "--recursive", "--versions", f"markhand/{bucket}"],
                        env=env_mc,
                    )
                )

                events = minio_inventory_events(inv_bytes)
                objects: list[dict[str, Any]] = []
                tombstones: list[dict[str, Any]] = []
                content_by_version: dict[tuple[str, str], tuple[int, str]] = {}
                for key, rows in events.items():
                    for row in rows:
                        version = str(
                            row.get("versionId") or row.get("version_id") or "null"
                        )
                        is_delete = bool(
                            row.get("deleteMarker") or row.get("isDeleteMarker")
                        )
                        if is_delete:
                            tombstones.append(
                                {
                                    "key": key,
                                    "deleteMarker": True,
                                    "checked": True,
                                }
                            )
                            continue
                        cmd = ["mc", "cat", f"markhand/{bucket}/{key}"]
                        if version and version != "null":
                            cmd = [
                                "mc",
                                "cat",
                                "--version-id",
                                version,
                                f"markhand/{bucket}/{key}",
                            ]
                        data = run(cmd, env=env_mc)
                        digest = hashlib.sha256(data).hexdigest()
                        safe = hashlib.sha256(f"{key}\0{version}".encode()).hexdigest()
                        out = work / "objects" / f"{safe}.bin"
                        out.write_bytes(data)
                        os.chmod(out, 0o600)
                        content_by_version[(key, version)] = (len(data), digest)
                        objects.append(
                            {
                                "key": key,
                                "versionId": version,
                                "objectSha256": digest,
                                "byteLength": len(data),
                                "bundleFile": f"objects/{safe}.bin",
                            }
                        )
                normalized = build_normalized_history(
                    events, content_by_version=content_by_version
                )
                (work / "minio-object-checksums.json").write_text(
                    json.dumps({"bundled": True, "objects": objects}, indent=2) + "\n",
                    encoding="utf-8",
                )
                (work / "minio-tombstones.json").write_text(
                    json.dumps({"tombstones": tombstones}, indent=2) + "\n",
                    encoding="utf-8",
                )
                (work / "minio-normalized-history.json").write_text(
                    json.dumps({"keys": normalized}, indent=2) + "\n",
                    encoding="utf-8",
                )

                snap_meta = json.loads(
                    urlopen(
                        Request(f"{qurl}/collections/{collection}/snapshots", method="POST"),
                        timeout=120,
                    )
                    .read()
                    .decode()
                )
                snap_name = (snap_meta.get("result") or {}).get("name")
                if not snap_name:
                    raise PipelineError("qdrant snapshot name missing")
                snap_bytes = urlopen(
                    f"{qurl}/collections/{collection}/snapshots/{snap_name}", timeout=120
                ).read()
                if not snap_bytes:
                    raise PipelineError("qdrant snapshot empty")
                (work / "qdrant-snapshot.bin").write_bytes(snap_bytes)
                (work / "qdrant-snapshot.name").write_text(snap_name + "\n", encoding="utf-8")
                (work / "qdrant-snapshot-create.json").write_text(
                    json.dumps(snap_meta, indent=2) + "\n", encoding="utf-8"
                )
                coll_info = json.loads(
                    urlopen(f"{qurl}/collections/{collection}", timeout=60).read()
                )
                points_count = int((coll_info.get("result") or {}).get("points_count") or 0)
                config_sha = hashlib.sha256(
                    json.dumps(coll_info.get("result") or {}, sort_keys=True).encode()
                ).hexdigest()
                payloads: list[Any] = []
                offset = None
                while True:
                    body: dict[str, Any] = {
                        "limit": 100,
                        "with_payload": True,
                        "with_vector": False,
                    }
                    if offset is not None:
                        body["offset"] = offset
                    req = Request(
                        f"{qurl}/collections/{collection}/points/scroll",
                        data=json.dumps(body).encode(),
                        headers={"Content-Type": "application/json"},
                        method="POST",
                    )
                    scroll = json.loads(urlopen(req, timeout=60).read())
                    result = scroll.get("result") or {}
                    batch = result.get("points") or []
                    for p in batch:
                        payloads.append({"id": p.get("id"), "payload": p.get("payload") or {}})
                    offset = result.get("next_page_offset")
                    if not offset or not batch:
                        break
                payload_ref_sha = hashlib.sha256(
                    json.dumps(payloads, sort_keys=True).encode()
                ).hexdigest()

                capture_end = int(time.time())
                (work / "capture-end.epoch").write_text(f"{capture_end}\n", encoding="utf-8")

                files = [
                    "postgres.dump",
                    "minio-versions.txt",
                    "minio-versions.jsonl",
                    "minio-object-checksums.json",
                    "minio-tombstones.json",
                    "minio-normalized-history.json",
                    "qdrant-snapshot-create.json",
                    "qdrant-snapshot.bin",
                    "qdrant-snapshot.name",
                    "WRITE_FENCE",
                    "fence-epoch.txt",
                    "capture-start.epoch",
                    "capture-end.epoch",
                ]
                checksums: dict[str, str] = {}
                sizes: dict[str, int] = {}
                for name in files:
                    data = (work / name).read_bytes()
                    checksums[name] = hashlib.sha256(data).hexdigest()
                    sizes[name] = len(data)
                    os.chmod(work / name, 0o600)
                for entry in objects:
                    rel = entry["bundleFile"]
                    data = (work / rel).read_bytes()
                    checksums[rel] = hashlib.sha256(data).hexdigest()
                    sizes[rel] = len(data)

                cross = json.loads(os.environ.get("MARKHAND_CROSS_STORE_REFS_JSON") or "[]")
                app = "unknown"
                try:
                    app = subprocess.check_output(
                        ["git", "-C", str(ROOT), "rev-parse", "--short", "HEAD"],
                        text=True,
                    ).strip()
                except Exception:
                    pass
                images: dict[str, Any] = {}
                lock = ROOT / "deploy/poc/images.lock.json"
                if lock.is_file():
                    images = json.loads(lock.read_text()).get("images", {})

                payload = {
                    "schemaVersion": SCHEMA_VERSION,
                    "capturedAt": stamp,
                    "fenceEpoch": epoch,
                    "fenceSetAt": fence_set_at,
                    "captureStartEpoch": capture_start,
                    "captureEndEpoch": capture_end,
                    "appVersion": app,
                    "migrationVersion": migrations[-1],
                    "mode": "blue_green",
                    "fence": "WRITE_FENCE",
                    "opsFence": "restore",
                    "opsFenceMandatory": True,
                    "sourceIds": {
                        "pgSystemIdentifier": identity["pgSystemIdentifier"],
                        "pgDatabase": identity["pgDatabase"],
                        "minioEndpoint": endpoint,
                        "minioBucket": bucket,
                        "qdrantUrl": qurl,
                        "qdrantCollection": collection,
                    },
                    "postgres": {
                        "dump": "postgres.dump",
                        "migrations": migrations,
                        "systemIdentifier": identity["pgSystemIdentifier"],
                        "database": identity["pgDatabase"],
                    },
                    "minio": {
                        "versioning": "Enabled",
                        "inventory": "minio-versions.jsonl",
                        "objects": "minio-object-checksums.json",
                        "tombstones": "minio-tombstones.json",
                    },
                    "qdrant": {
                        "snapshot": "qdrant-snapshot.bin",
                        "collection": collection,
                        "pointsCount": points_count,
                        "configSha256": config_sha,
                        "payloadRefSha256": payload_ref_sha,
                    },
                    "toolVersions": {
                        "pg_dump": "pg_dump",
                        "mc": "minio/mc:env-MC_HOST",
                        "appGit": app,
                    },
                    "imageVersions": images,
                    "artifactSha256": checksums,
                    "artifactBytes": sizes,
                    "crossStoreRefs": cross,
                    "watermarks": {
                        "pgWalLsn": lsn,
                        "fenceEpoch": epoch,
                        "jobsDrained": True,
                        "writeGate": write_gate_label,
                    },
                    "trustedBoundary": {
                        "mode": "hmac_sha256",
                        "keyId": key_id,
                        "signatureFile": "manifest.sig",
                        "note": "HMAC-SHA256 over raw manifest.json; key via env only",
                    },
                    "rpoSecondsTarget": 900,
                    "queryReadyRtoSecondsTarget": 3600,
                    "status": "captured",
                }
                write_signed_manifest(work, payload)
                if dest.exists():
                    raise PipelineError(f"destination exists: {dest}")
                work.rename(dest)
                return dest
            finally:
                session.unlock(ADVISORY_LOCK_KEY)
    finally:
        os.umask(old_umask)


def _minio_bucket_exists(env_mc: dict[str, str], bucket: str) -> bool:
    proc = subprocess.run(
        ["mc", "ls", f"markhand/{bucket}"],
        check=False,
        capture_output=True,
        env=env_mc,
    )
    return proc.returncode == 0


def _qdrant_collection_exists(qurl: str, collection: str) -> bool:
    try:
        code = urlopen(f"{qurl}/collections/{collection}", timeout=30).status
        return 200 <= int(code) < 300
    except Exception:
        return False


def restore_green(backup_dir: Path) -> None:
    """Restore only to isolated green targets. Promote/cutover is disabled."""
    old_umask = _umask_secure()
    try:
        require_signing_env()
        blue_url = os.environ["DATABASE_URL"]
        green_url = os.environ["MARKHAND_GREEN_DATABASE_URL"]
        green_bucket = os.environ["MARKHAND_GREEN_MINIO_BUCKET"]
        green_coll = os.environ["MARKHAND_GREEN_QDRANT_COLLECTION"]
        endpoint = os.environ["MINIO_ENDPOINT"]
        qurl = os.environ["QDRANT_URL"].rstrip("/")
        green_db = db_name_from_url(green_url)

        # Auth + schema before any mutation.
        manifest, _raw = load_authenticated_manifest(backup_dir)
        verify_artifacts(backup_dir, manifest)

        allowlists = GreenAllowlists.load_from_env()
        allowlist_digest = allowlists.digest

        blue_id = read_identity(blue_url)
        # Cluster system id from blue (green DB may be absent — exclusive create).
        allowlists.assert_pg(blue_id["pgSystemIdentifier"], green_db)
        allowlists.assert_bucket(green_bucket)
        allowlists.assert_collection(green_coll)
        src = manifest["sourceIds"]
        assert_not_blue_alias(
            blue_bucket=src["minioBucket"],
            green_bucket=green_bucket,
            blue_collection=src["qdrantCollection"],
            green_collection=green_coll,
            blue_endpoint=src["minioEndpoint"],
            green_endpoint=endpoint,
            blue_qdrant=src["qdrantUrl"],
            green_qdrant=qurl,
        )
        if GreenAllowlists.load_from_env().digest != allowlist_digest:
            raise PipelineError("allowlists mutated during preflight; refuse")

        env_mc = mc_env_for(endpoint)
        # Existing allowlisted targets must fail BEFORE any mutation.
        exists_db = psql(
            maintenance_url(blue_url),
            f"SELECT 1 FROM pg_database WHERE datname='{green_db}';",
        )
        if exists_db == "1":
            raise PipelineError(
                "REFUSING_EXISTING_ALLOWLISTED_TARGET: green database already exists"
            )
        if _minio_bucket_exists(env_mc, green_bucket):
            raise PipelineError(
                "REFUSING_EXISTING_ALLOWLISTED_TARGET: green MinIO bucket already exists"
            )
        if _qdrant_collection_exists(qurl, green_coll):
            raise PipelineError(
                "REFUSING_EXISTING_ALLOWLISTED_TARGET: green Qdrant collection already exists"
            )

        token = CreationToken.issue(backup_dir)
        token.write_resource_marker(
            backup_dir / "green-resources.marker",
            {
                "pgDatabase": green_db,
                "minioBucket": green_bucket,
                "qdrantCollection": green_coll,
                "pgSystemIdentifier": blue_id["pgSystemIdentifier"],
            },
        )
        token.assert_resource_marker(backup_dir / "green-resources.marker")

        # Exclusive creates (no ignore-existing, no pre-delete).
        psql(
            maintenance_url(blue_url),
            f'CREATE DATABASE "{green_db}" OWNER markhand;',
        )
        green_id = read_identity(green_url)
        if green_id["pgDatabase"] != green_db:
            raise PipelineError("green database identity mismatch after create")
        allowlists.assert_pg(green_id["pgSystemIdentifier"], green_id["pgDatabase"])

        with private_pg_env(green_url) as (safe_green, green_env):
            run(
                [
                    "pg_restore",
                    "--clean",
                    "--if-exists",
                    "--no-owner",
                    f"--dbname={safe_green}",
                    str(backup_dir / "postgres.dump"),
                ],
                env=green_env,
            )

        got_migs = psql(
            green_url,
            "SELECT coalesce(string_agg(name, ',' ORDER BY name), '') "
            "FROM markhand_schema_migrations;",
        ).split(",")
        got_migs = [m for m in got_migs if m]
        if got_migs != manifest["postgres"]["migrations"]:
            raise PipelineError("green migrations != manifest postgres.migrations")

        app_exists = psql(green_url, "SELECT 1 FROM pg_roles WHERE rolname='markhand_app';")
        if app_exists != "1":
            raise PipelineError("markhand_app role missing on green")
        fence_active = psql(
            green_url,
            "SET ROLE markhand_app; SELECT markhand_any_blocking_fence_active();",
        ).splitlines()[-1]
        if fence_active not in {"t", "true"}:
            raise PipelineError("app-role fence probe expected true")

        rls = psql(
            green_url,
            """
            SELECT string_agg(relname||':'||relrowsecurity||':'||relforcerowsecurity, ',' ORDER BY relname)
            FROM pg_class
            WHERE relname IN ('documents','chunks','document_versions','jobs','collections');
            """,
        )
        for table in ("documents", "chunks", "document_versions", "jobs", "collections"):
            if not re.search(rf"{table}:(t|true):(t|true)", rls, re.I):
                raise PipelineError(f"RLS/FORCE missing for {table}")

        run(["mc", "mb", f"markhand/{green_bucket}"], env=env_mc)
        run(["mc", "version", "enable", f"markhand/{green_bucket}"], env=env_mc)

        objects = json.loads((backup_dir / "minio-object-checksums.json").read_text())[
            "objects"
        ]
        history = json.loads((backup_dir / "minio-normalized-history.json").read_text())[
            "keys"
        ]
        used_obj: set[int] = set()
        for key_entry in history:
            key = key_entry["key"]
            for ev in key_entry["events"]:
                if ev["type"] == "delete":
                    run(
                        ["mc", "rm", "--force", f"markhand/{green_bucket}/{key}"],
                        env=env_mc,
                    )
                    continue
                if ev["type"] != "put":
                    raise PipelineError(f"unknown MinIO event type {ev.get('type')}")
                digest = ev["contentSha256"]
                entry = None
                for idx, cand in enumerate(objects):
                    if idx in used_obj:
                        continue
                    if cand["key"] == key and cand["objectSha256"] == digest:
                        entry = cand
                        used_obj.add(idx)
                        break
                if entry is None:
                    raise PipelineError(
                        f"missing intermediate bundled object for {key} sha={digest}"
                    )
                data = (backup_dir / entry["bundleFile"]).read_bytes()
                if len(data) != int(ev["size"]) or hashlib.sha256(data).hexdigest() != digest:
                    raise PipelineError(f"object pre-put mismatch {key}")
                run(
                    ["mc", "pipe", f"markhand/{green_bucket}/{key}"],
                    input_bytes=data,
                    env=env_mc,
                )

        # Rebuild green normalized history (content hashes) and compare.
        green_inv = run(
            ["mc", "ls", "--recursive", "--json", "--versions", f"markhand/{green_bucket}"],
            env=env_mc,
        )
        green_events = minio_inventory_events(green_inv)
        green_content: dict[tuple[str, str], tuple[int, str]] = {}
        for key, rows in green_events.items():
            for row in rows:
                version = str(row.get("versionId") or row.get("version_id") or "null")
                if row.get("deleteMarker") or row.get("isDeleteMarker"):
                    continue
                cmd = ["mc", "cat", "--version-id", version, f"markhand/{green_bucket}/{key}"]
                if version == "null":
                    cmd = ["mc", "cat", f"markhand/{green_bucket}/{key}"]
                data = run(cmd, env=env_mc)
                green_content[(key, version)] = (
                    len(data),
                    hashlib.sha256(data).hexdigest(),
                )
        green_norm = build_normalized_history(green_events, content_by_version=green_content)
        compare_normalized_history(history, green_norm)

        # Live byte-for-byte for keys whose last event is put.
        for key_entry in history:
            key = key_entry["key"]
            last = None
            for ev in key_entry["events"]:
                last = ev
            if last and last["type"] == "put":
                got = run(["mc", "cat", f"markhand/{green_bucket}/{key}"], env=env_mc)
                if hashlib.sha256(got).hexdigest() != last["contentSha256"]:
                    raise PipelineError(f"byte-for-byte mismatch {key}")
                if len(got) != int(last["size"]):
                    raise PipelineError(f"live size mismatch {key}")

        # Qdrant: collection must still be absent — upload creates it (no DELETE).
        if _qdrant_collection_exists(qurl, green_coll):
            raise PipelineError("green Qdrant collection appeared before exclusive create")
        run(
            [
                "curl",
                "-fsS",
                "-X",
                "POST",
                f"{qurl}/collections/{green_coll}/snapshots/upload?priority=snapshot",
                "-F",
                f"snapshot=@{backup_dir / 'qdrant-snapshot.bin'}",
                "-o",
                str(backup_dir / "qdrant-green-upload.json"),
            ]
        )
        coll_info = json.loads(urlopen(f"{qurl}/collections/{green_coll}", timeout=60).read())
        points = int((coll_info.get("result") or {}).get("points_count") or 0)
        if points != int(manifest["qdrant"]["pointsCount"]):
            raise PipelineError(
                f"qdrant points_count mismatch got={points} want={manifest['qdrant']['pointsCount']}"
            )
        payloads: list[Any] = []
        offset = None
        while True:
            body: dict[str, Any] = {"limit": 100, "with_payload": True, "with_vector": False}
            if offset is not None:
                body["offset"] = offset
            req = Request(
                f"{qurl}/collections/{green_coll}/points/scroll",
                data=json.dumps(body).encode(),
                headers={"Content-Type": "application/json"},
                method="POST",
            )
            scroll = json.loads(urlopen(req, timeout=60).read())
            result = scroll.get("result") or {}
            batch = result.get("points") or []
            for p in batch:
                payloads.append({"id": p.get("id"), "payload": p.get("payload") or {}})
            offset = result.get("next_page_offset")
            if not offset or not batch:
                break
        payload_sha = hashlib.sha256(json.dumps(payloads, sort_keys=True).encode()).hexdigest()
        if payload_sha != manifest["qdrant"]["payloadRefSha256"]:
            raise PipelineError("qdrant payloadRefSha256 mismatch")

        for ref in manifest.get("crossStoreRefs") or []:
            doc = str(ref.get("documentId") or "")
            key = str(ref.get("objectKey") or "")
            if not doc or not key:
                continue
            if not re.fullmatch(r"[0-9a-f-]{36}", doc) or not re.fullmatch(
                r"[A-Za-z0-9._/-]+", key
            ):
                raise PipelineError("cross-store ref malformed")
            count = _psql_vars(
                green_url,
                "SELECT count(*) FROM document_versions "
                "WHERE document_id = :'doc'::uuid AND original_object_key = :'key';",
                {"doc": doc, "key": key},
            )
            if count != "1":
                raise PipelineError("cross-store ref missing in green PG")

        print(
            json.dumps(
                {
                    "status": "RESTORE_GREEN_OK",
                    "promote": "DISABLED",
                    "greenIdentity": green_id,
                    "greenBucket": green_bucket,
                    "greenCollection": green_coll,
                    "allowlistDigest": allowlist_digest,
                    "creationToken": token.token,
                }
            )
        )
    finally:
        os.umask(old_umask)


def promote_disabled(_backup_dir: Path) -> None:
    raise PipelineError(
        "PROMOTE_DISABLED_UNTIL_API_CONSUMES_ROUTING_AND_INDEPENDENT_DURABLE_"
        "RECONCILE_TARGET_STATE_ATTESTATION: cutover/promote removed to avoid "
        "false traffic switch / partial state"
    )


def main(argv: list[str]) -> int:
    try:
        if len(argv) < 2:
            print(
                "usage: pipeline.py capture|restore-green|promote ...",
                file=sys.stderr,
            )
            return 2
        cmd = argv[1]
        if cmd == "capture":
            dest = Path(argv[2] if len(argv) > 2 else os.environ["MARKHAND_BACKUP_DEST"])
            out = capture(dest)
            print(out)
            return 0
        if cmd == "restore-green":
            restore_green(Path(argv[2]))
            return 0
        if cmd in {"cutover", "promote"}:
            promote_disabled(Path(argv[2]) if len(argv) > 2 else Path("."))
            return 1
        print("unknown command", file=sys.stderr)
        return 2
    except (PipelineError, ManifestError, PgIdentityError, TargetError, PgSessionError) as exc:
        print(f"pipeline_error: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
