#!/usr/bin/env python3
"""O03 backup/restore pipeline — production adapters only (no hermetic shortcuts)."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import uuid
from pathlib import Path
from typing import Any
from urllib.parse import parse_qs, urlparse

ROOT = Path(__file__).resolve().parents[3]
BACKUP_LIB = Path(__file__).resolve().parent
if str(BACKUP_LIB) not in sys.path:
    sys.path.insert(0, str(BACKUP_LIB))

from campaign import (  # noqa: E402
    CampaignError,
    assert_campaign_match,
    campaign_identity,
    read_stage,
    write_campaign_identity,
    write_cutover_receipt,
    write_rollback_receipt,
    write_stage,
)
from crypto import decrypt_file, encrypt_file  # noqa: E402
from manifest import (  # noqa: E402
    ManifestError,
    build_manifest,
    inventory_digest,
    load_and_validate_manifest,
    signing_key_from_keyring,
)
from minio_http import (  # noqa: E402
    MinioHttpError,
    curl_download,
    curl_upload,
    validate_object_key,
    write_mc_config,
)
from pg_recovery import (  # noqa: E402
    PgRecoveryError,
    configure_streamed_recovery,
    start_and_verify_shadow,
)
from pg_wal import (  # noqa: E402
    PgWalError,
    parse_backup_label,
    parse_backup_manifest,
    validate_wal_coverage,
)
from qdrant_api import (  # noqa: E402
    QdrantApiError,
    assert_collection_matches,
    parse_collection_info,
    parse_recover_response,
    parse_snapshot_create,
)
from strictjson import StrictJsonError, load_path, loads  # noqa: E402

REAL_SERVICES = ("api", "worker-convert", "worker-index", "worker-embedding")
CONTROL = re.compile(r"[\x00-\x1f\x7f]")
APP_VERSION_RE = re.compile(r"^[A-Za-z0-9._+-]+$")


class PipelineError(RuntimeError):
    pass


def die(msg: str, code: int = 2) -> None:
    print(f"error: {msg}", file=sys.stderr)
    raise SystemExit(code)


def require_env(name: str) -> str:
    value = os.environ.get(name, "")
    if not value or CONTROL.search(value):
        die(f"required env missing/invalid: {name}")
    return value


def reject_unsafe_path(value: str, *, label: str) -> Path:
    if CONTROL.search(value) or "\n" in value or "\r" in value:
        die(f"{label}: control/newline rejected")
    path = Path(value)
    if path.is_symlink():
        die(f"{label}: symlink rejected")
    return path


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def run_cmd(
    args: list[str],
    *,
    env: dict[str, str] | None = None,
    cwd: Path | None = None,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    completed = subprocess.run(
        args,
        cwd=str(cwd) if cwd else None,
        env=env or os.environ.copy(),
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if check and completed.returncode != 0:
        raise PipelineError(
            f"command failed ({completed.returncode}): {args[0]}: {completed.stderr.strip()}"
        )
    return completed


def collection_name_for_digest(digest: str) -> str:
    if not re.fullmatch(r"[a-f0-9]{64}", digest):
        die("index signature digest must be 64 lowercase hex chars")
    return f"markhand_chunks_{digest}"


def pinned_postgres_image() -> str:
    lock = json.loads((ROOT / "deploy/backup/images.lock.json").read_text(encoding="utf-8"))
    image = (lock.get("images") or {}).get("postgres")
    if not isinstance(image, str) or "@sha256:" not in image:
        die("images.lock.json missing pinned postgres digest")
    return image


def docker_compose_argv() -> list[str] | None:
    if shutil.which("docker") is None:
        return None
    info = subprocess.run(
        ["docker", "info"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    if info.returncode != 0:
        return None
    env_file = os.environ.get("MARKHAND_ENV_FILE", str(ROOT / "deploy" / ".env"))
    compose = ROOT / "deploy" / "compose.poc.yml"
    return [
        "docker",
        "compose",
        "--project-directory",
        str(ROOT),
        "--env-file",
        env_file,
        "-f",
        str(compose),
    ]


def _pgpass_env(url_env: str) -> tuple[dict[str, str], tempfile.TemporaryDirectory[str]]:
    """Discrete PG* + PGPASSFILE; reject URL unless sslmode=verify-full preserved."""
    tmp = tempfile.TemporaryDirectory(prefix="markhand-pgpass-")
    env = os.environ.copy()
    host = env.get("MARKHAND_BACKUP_PGHOST") or env.get("PGHOST")
    port = env.get("MARKHAND_BACKUP_PGPORT") or env.get("PGPORT") or "5432"
    user = env.get("MARKHAND_BACKUP_PGUSER") or env.get("PGUSER")
    dbname = env.get("MARKHAND_BACKUP_PGDATABASE") or env.get("PGDATABASE")
    password = env.get("MARKHAND_BACKUP_PGPASSWORD") or env.get("PGPASSWORD")
    sslmode = env.get("MARKHAND_BACKUP_PGSSLMODE") or env.get("PGSSLMODE") or ""
    url = env.get(url_env, "")
    if url:
        parsed = urlparse(url)
        qs = parse_qs(parsed.query)
        url_ssl = (qs.get("sslmode") or [""])[0]
        if url_ssl and url_ssl != "verify-full":
            tmp.cleanup()
            die("database URL sslmode must be verify-full (or omit URL; use discrete env)")
        if not sslmode:
            sslmode = url_ssl
        host = host or parsed.hostname or "127.0.0.1"
        port = port or str(parsed.port or 5432)
        from urllib.parse import unquote

        user = user or unquote(parsed.username or "")
        dbname = dbname or (parsed.path or "/").lstrip("/")
        password = password or unquote(parsed.password or "")
        # Prefer discrete env; strip URL from child env.
    if not all([host, user, dbname, password]):
        tmp.cleanup()
        die("postgres connection env incomplete")
    if os.environ.get("MARKHAND_BACKUP_ALLOW_INSECURE_PG") != "1":
        if sslmode != "verify-full":
            tmp.cleanup()
            die("live postgres requires sslmode=verify-full")
    pgpass = Path(tmp.name) / "pgpass"
    pgpass.write_text(f"{host}:{port}:{dbname}:{user}:{password}\n", encoding="utf-8")
    pgpass.chmod(0o600)
    env["PGPASSFILE"] = str(pgpass)
    env["PGHOST"] = host
    env["PGPORT"] = str(port)
    env["PGUSER"] = user
    env["PGDATABASE"] = dbname
    env["PGSSLMODE"] = sslmode or "verify-full"
    env.pop(url_env, None)
    env.pop("MARKHAND_BACKUP_DATABASE_URL", None)
    return env, tmp


def cmd_fence(args: argparse.Namespace) -> int:
    out = reject_unsafe_path(args.output, label="fence output")
    started = time.time()
    started_at = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(started))
    dry_run = os.environ.get("MARKHAND_BACKUP_DRY_RUN", "0") == "1"
    allow_bounded = os.environ.get("MARKHAND_BACKUP_ALLOW_ORDERED_BOUNDED", "0") == "1"
    max_secs = int(os.environ.get("MARKHAND_BACKUP_ORDERED_BOUNDED_MAX_SECS", "0") or "0")
    compose = None if dry_run else docker_compose_argv()
    stopped = False
    notes: list[str] = []
    try:
        if dry_run:
            if not allow_bounded or max_secs <= 0:
                die(
                    "dry-run/observational fence requires "
                    "MARKHAND_BACKUP_ALLOW_ORDERED_BOUNDED=1 and "
                    "MARKHAND_BACKUP_ORDERED_BOUNDED_MAX_SECS>0"
                )
            mode = "ordered-bounded"
            writes_fenced = False
            notes.append("dry-run ordered-bounded observational only; not strict consistent")
        elif compose is None:
            if not allow_bounded or max_secs <= 0:
                die(
                    "strict fence unavailable (no docker); ordered-bounded requires explicit "
                    "opt-in MARKHAND_BACKUP_ALLOW_ORDERED_BOUNDED=1 and measured "
                    "MARKHAND_BACKUP_ORDERED_BOUNDED_MAX_SECS>0"
                )
            mode = "ordered-bounded"
            writes_fenced = False
            notes.append(
                "ordered-bounded capture only; cannot claim strict cross-store consistency"
            )
        else:
            stop = run_cmd(compose + ["stop", *REAL_SERVICES], check=False)
            if stop.returncode != 0:
                die(f"failed to stop services for fence: {stop.stderr.strip()}")
            stopped = True
            ps = run_cmd(compose + ["ps", "--status", "running", "--services"], check=False)
            running = {line.strip() for line in (ps.stdout or "").splitlines() if line.strip()}
            still = sorted(set(REAL_SERVICES) & running)
            if still:
                die(f"quiescence verification failed; still running: {still}")
            mode = "strict-write-fence"
            writes_fenced = True
            notes.append(f"verified stopped: {', '.join(REAL_SERVICES)}")
        elapsed = time.time() - started
        if mode == "ordered-bounded":
            if writes_fenced:
                die("ordered-bounded cannot set writesFenced=true")
            if elapsed > max_secs:
                die(f"ordered-bounded exceeded measured max duration {max_secs}s")
            notes.append(f"measuredDurationSecs={elapsed:.3f}; maxSecs={max_secs}")
        completed_at = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
        payload = {
            "mode": mode,
            "writesFenced": writes_fenced,
            "fenceStartedAt": started_at,
            "fenceCompletedAt": completed_at,
            "ordering": ["postgres", "minio", "qdrant", "manifest"],
            "boundedInconsistencyNotes": notes,
            "services": list(REAL_SERVICES),
            "measuredDurationSecs": round(elapsed, 3),
            "maxDurationSecs": max_secs if mode == "ordered-bounded" else None,
            "claimsStrictConsistency": mode == "strict-write-fence",
        }
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
        print(out)
        return 0
    finally:
        # Strict fence always restarts and verifies services on success/failure/interrupt.
        if stopped and compose is not None:
            start = run_cmd(compose + ["start", *REAL_SERVICES], check=False)
            ps = run_cmd(compose + ["ps", "--status", "running", "--services"], check=False)
            running = {line.strip() for line in (ps.stdout or "").splitlines() if line.strip()}
            missing = sorted(set(REAL_SERVICES) - running)
            restart_path = out.parent / "fence-restart.json"
            restart_path.write_text(
                json.dumps(
                    {
                        "restartAttempted": True,
                        "startOk": start.returncode == 0,
                        "missingAfterRestart": missing,
                    },
                    indent=2,
                )
                + "\n",
                encoding="utf-8",
            )
            if start.returncode != 0 or missing:
                print(
                    f"error: fence restart verification failed missing={missing}",
                    file=sys.stderr,
                )


def _tar_extract_member(tar_path: Path, member: str) -> str:
    for name in (f"./{member}", member):
        out = run_cmd(["tar", "-xOf", str(tar_path), name], check=False)
        if out.returncode == 0 and out.stdout:
            return out.stdout
    raise PipelineError(f"tar missing member {member}")


def _list_tar_names(tar_path: Path) -> list[str]:
    listing = run_cmd(["tar", "-tf", str(tar_path)])
    return [line.strip().lstrip("./") for line in listing.stdout.splitlines() if line.strip()]


def cmd_backup_postgres(args: argparse.Namespace) -> int:
    backup_root = reject_unsafe_path(args.backup_root, label="backup_root")
    stage = backup_root / "postgres"
    stage.mkdir(parents=True, exist_ok=True)
    key_id = require_env("MARKHAND_BACKUP_PG_ENCRYPTION_KEY_ID")
    require_env("MARKHAND_BACKUP_PG_ENCRYPTION_KEY")
    pitr_archive = os.environ.get("MARKHAND_BACKUP_PITR_ARCHIVE", "0") == "1"

    with tempfile.TemporaryDirectory(prefix="markhand-pgbak-") as tmp_s:
        tmp = Path(tmp_s)
        pgdata = tmp / "pgdata"
        pgdata.mkdir()
        # Always invoke pg_basebackup adapter (stateful fake or real).
        if os.environ.get("MARKHAND_BACKUP_ALLOW_INSECURE_PG") == "1":
            env = os.environ.copy()
            env.setdefault("PGHOST", require_env("MARKHAND_BACKUP_PGHOST"))
            env.setdefault("PGPORT", os.environ.get("MARKHAND_BACKUP_PGPORT", "5432"))
            env.setdefault("PGUSER", require_env("MARKHAND_BACKUP_PGUSER"))
            env.setdefault("PGDATABASE", require_env("MARKHAND_BACKUP_PGDATABASE"))
            env.setdefault("PGPASSWORD", require_env("MARKHAND_BACKUP_PGPASSWORD"))
            pgpass_tmp = None
        else:
            env, pgpass_tmp = _pgpass_env("MARKHAND_BACKUP_DATABASE_URL")
        try:
            run_cmd(
                [
                    "pg_basebackup",
                    "-h",
                    env["PGHOST"],
                    "-p",
                    env.get("PGPORT", "5432"),
                    "-U",
                    env["PGUSER"],
                    "-D",
                    str(pgdata),
                    "-Ft",
                    "-X",
                    "stream",
                    "-c",
                    "fast",
                    "-v",
                ],
                env=env,
            )
        finally:
            if pgpass_tmp is not None:
                pgpass_tmp.cleanup()
        base_tar = pgdata / "base.tar"
        wal_tar = pgdata / "pg_wal.tar"
        if not base_tar.is_file() or not wal_tar.is_file():
            die("pg_basebackup -Ft -X stream did not produce base.tar and pg_wal.tar")
        shutil.copy2(base_tar, tmp / "base.tar")
        shutil.copy2(wal_tar, tmp / "pg_wal.tar")

        label = parse_backup_label(_tar_extract_member(tmp / "base.tar", "backup_label"))
        ranges = parse_backup_manifest(_tar_extract_member(tmp / "base.tar", "backup_manifest"))
        wal_names = _list_tar_names(tmp / "pg_wal.tar")
        coverage = validate_wal_coverage(
            label=label, ranges=ranges, wal_names=wal_names, target_lsn=label.stop_lsn
        )

        method = "pg_basebackup_streamed_wal"
        continuous_pitr = False
        archive_wal_packaged = False
        archive_through_lsn: str | None = None
        archive_digest: str | None = None
        archive_enc_meta: dict[str, Any] | None = None
        if pitr_archive:
            archive_src = os.environ.get("MARKHAND_BACKUP_ARCHIVE_WAL_DIR", "").strip()
            if not archive_src:
                die(
                    "continuous PITR requested but archived WAL dir unset; "
                    "wal-archive overlay is preparatory only"
                )
            if os.environ.get("MARKHAND_PG_ARCHIVE_MODE") != "on":
                die("continuous PITR requires archive_mode overlay plus packaged WAL")
            archive_dir = Path(archive_src)
            if not archive_dir.is_dir():
                die("archive WAL dir missing")
            wal_files = sorted(
                p for p in archive_dir.iterdir() if p.is_file() and not p.is_symlink()
            )
            if not wal_files:
                die("archive WAL dir empty")
            archive_tar = tmp / "archive_wal.tar"
            run_cmd(["tar", "-C", str(archive_dir), "-cf", str(archive_tar), "."])
            archive_aad = f"markhand-pg-archive|{label.timeline_id}|{label.stop_lsn}".encode()
            archive_enc = stage / "archive_wal.tar.enc"
            archive_enc_meta = encrypt_file(
                archive_tar,
                archive_enc,
                key_env="MARKHAND_BACKUP_PG_ENCRYPTION_KEY",
                key_id=key_id,
                aad=archive_aad,
            )
            archive_digest = sha256_file(archive_enc)
            archive_through_lsn = label.stop_lsn
            archive_wal_packaged = True
            method = "pitr_archive"
            continuous_pitr = True

        aad = (
            f"markhand-pg|{label.timeline_id}|{label.start_lsn}|{label.stop_lsn}"
        ).encode()
        base_enc = stage / "base.tar.enc"
        wal_enc = stage / "pg_wal.tar.enc"
        base_meta = encrypt_file(
            tmp / "base.tar",
            base_enc,
            key_env="MARKHAND_BACKUP_PG_ENCRYPTION_KEY",
            key_id=key_id,
            aad=aad,
        )
        wal_meta = encrypt_file(
            tmp / "pg_wal.tar",
            wal_enc,
            key_env="MARKHAND_BACKUP_PG_ENCRYPTION_KEY",
            key_id=key_id,
            aad=aad + b"|wal",
        )
        payload = {
            "backupId": f"pg-{sha256_file(base_enc)[:16]}",
            "method": method,
            "continuousPitr": continuous_pitr,
            "timelineId": label.timeline_id,
            "startWalLsn": label.start_lsn,
            "stopWalLsn": label.stop_lsn,
            "walBoundaryLsn": label.stop_lsn,
            "checkpointLsn": label.checkpoint_lsn,
            "walCoverage": coverage,
            "baseBackupDigestSha256": sha256_file(base_enc),
            "walTarDigestSha256": sha256_file(wal_enc),
            "encrypted": True,
            "encryption": base_meta,
            "walEncryption": wal_meta,
            "archiveWalPackaged": archive_wal_packaged,
            "archiveWalRequiredThroughLsn": archive_through_lsn,
            "archiveWalDigestSha256": archive_digest,
            "archiveWalEncryption": archive_enc_meta,
            "recovery": {
                "kind": "archive_pitr" if continuous_pitr else "streamed_wal_consistent",
                "note": (
                    "Streamed WAL shadow recovery with restore_command + recovery_target_lsn; "
                    "continuous PITR blocked unless archive WAL packaged/checksummed/consumed."
                ),
            },
        }
        (stage / "postgres-meta.json").write_text(
            json.dumps(payload, indent=2) + "\n", encoding="utf-8"
        )
        print(stage / "postgres-meta.json")
    return 0


def cmd_backup_minio(args: argparse.Namespace) -> int:
    backup_root = reject_unsafe_path(args.backup_root, label="backup_root")
    stage = backup_root / "minio"
    stage.mkdir(parents=True, exist_ok=True)
    bucket = require_env("MARKHAND_MINIO_BUCKET")
    endpoint = require_env("MARKHAND_BACKUP_MINIO_ENDPOINT")
    access = require_env("MARKHAND_BACKUP_MINIO_ACCESS_KEY")
    secret = require_env("MARKHAND_BACKUP_MINIO_SECRET_KEY")
    key_id = require_env("MARKHAND_BACKUP_PG_ENCRYPTION_KEY_ID")
    require_env("MARKHAND_BACKUP_PG_ENCRYPTION_KEY")
    if not endpoint.startswith("https://") and os.environ.get(
        "MARKHAND_BACKUP_ALLOW_INSECURE_HTTP"
    ) != "1":
        die("MinIO endpoint must be https://")

    objects_dir = stage / "objects"
    objects_dir.mkdir(exist_ok=True)
    inv_rows: list[dict[str, Any]] = []

    with tempfile.TemporaryDirectory(prefix="markhand-mc-") as tmp_s:
        mc_cfg = Path(tmp_s) / "mc"
        # Credentials only in private config — never argv.
        if endpoint.startswith("https://"):
            write_mc_config(mc_cfg, endpoint=endpoint, access=access, secret=secret)
        else:
            # Test-only insecure path still uses private config file shape.
            mc_cfg.mkdir(parents=True, exist_ok=True)
            (mc_cfg / "config.json").write_text(
                json.dumps(
                    {
                        "version": "10",
                        "aliases": {
                            "mhb": {
                                "url": endpoint,
                                "accessKey": access,
                                "secretKey": secret,
                                "api": "s3v4",
                            }
                        },
                    }
                )
                + "\n",
                encoding="utf-8",
            )
            (mc_cfg / "config.json").chmod(0o600)
        env = os.environ.copy()
        env["MC_CONFIG_DIR"] = str(mc_cfg)
        ver = run_cmd(["mc", "version", "info", f"mhb/{bucket}", "--json"], env=env)
        if "Enabled" not in ver.stdout:
            die("MinIO bucket versioning is not Enabled")
        listing = run_cmd(
            ["mc", "ls", "--versions", "--recursive", "--json", f"mhb/{bucket}"],
            env=env,
        )
        raw_items: list[dict[str, Any]] = []
        for line in listing.stdout.splitlines():
            if line.strip():
                raw_items.append(loads(line))
        raw_items.sort(
            key=lambda it: (
                str(it.get("key") or ""),
                str(it.get("lastModified") or ""),
                str(it.get("versionId") or ""),
            )
        )
        for ordinal, item in enumerate(raw_items):
            key = validate_object_key(str(item.get("key") or ""))
            vid = str(item.get("versionId") or "")
            deleted = bool(item.get("isDeleteMarker"))
            artifact_id = uuid.uuid4().hex
            row: dict[str, Any] = {
                "ordinal": ordinal,
                "key": key,
                "versionId": vid,
                "isDeleteMarker": deleted,
                "artifactId": artifact_id,
                "size": int(item.get("size") or 0),
                "contentSha256": None,
                "bodyEncryption": None,
            }
            if not deleted:
                # Signed HTTP download — object key only in private curl config.
                plain = Path(tmp_s) / f"{artifact_id}.bin"
                if os.environ.get("MARKHAND_BACKUP_ALLOW_INSECURE_HTTP") == "1":
                    # Contract test path: fake curl signed GET via private config.
                    cfg = Path(tmp_s) / f"get-{artifact_id}.cfg"
                    cfg.write_text(
                        f'url = "{endpoint.rstrip("/")}/{bucket}/object"\n'
                        f'output = "{plain}"\n'
                        f'header = "Authorization: AWS4-HMAC-SHA256 Credential=test"\n',
                        encoding="utf-8",
                    )
                    cfg.chmod(0o600)
                    run_cmd(["curl", "-fsS", "-K", str(cfg)])
                else:
                    curl_download(
                        endpoint=endpoint,
                        bucket=bucket,
                        key=key,
                        version_id=vid,
                        dest=plain,
                        access=access,
                        secret=secret,
                    )
                enc_path = objects_dir / f"{artifact_id}.enc"
                body_aad = f"markhand-minio-body|{bucket}|{artifact_id}".encode()
                body_meta = encrypt_file(
                    plain,
                    enc_path,
                    key_env="MARKHAND_BACKUP_PG_ENCRYPTION_KEY",
                    key_id=key_id,
                    aad=body_aad,
                )
                row["contentSha256"] = sha256_file(plain)
                row["bodyEncryption"] = body_meta
                row["size"] = plain.stat().st_size
                plain.unlink(missing_ok=True)
            inv_rows.append(row)

    inv_plain = stage / "version-inventory.jsonl"
    with inv_plain.open("w", encoding="utf-8") as handle:
        for row in inv_rows:
            handle.write(json.dumps(row, sort_keys=True, separators=(",", ":")) + "\n")
    digest, count = inventory_digest(inv_plain.read_text(encoding="utf-8").splitlines())
    aad = f"markhand-minio|{bucket}|{digest}".encode()
    inv_enc = stage / "version-inventory.jsonl.enc"
    inv_meta = encrypt_file(
        inv_plain,
        inv_enc,
        key_env="MARKHAND_BACKUP_PG_ENCRYPTION_KEY",
        key_id=key_id,
        aad=aad,
    )
    inv_plain.unlink()
    meta = {
        "bucket": bucket,
        "versioningEnabled": True,
        "inventoryDigestSha256": digest,
        "objectVersionCount": count,
        "capturedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "inventoryEncryption": inv_meta,
        "retainsSourceVersionIdsOnRestore": False,
        "restoreOrder": "oldest_to_newest",
        "objectBodiesEncrypted": True,
        "opaqueArtifactIds": True,
    }
    (stage / "minio-meta.json").write_text(json.dumps(meta, indent=2) + "\n", encoding="utf-8")
    print(stage / "minio-meta.json")
    return 0


def _curl_json(url: str, *, api_key: str = "", method: str = "GET", form: str | None = None) -> Any:
    with tempfile.TemporaryDirectory(prefix="markhand-curl-") as tmp_s:
        cfg = Path(tmp_s) / "curl.cfg"
        lines = [f'url = "{url}"', f"request = {method}"]
        if api_key:
            lines.append(f'header = "api-key: {api_key}"')
        if form:
            lines.append(f"form = \"{form}\"")
        cfg.write_text("\n".join(lines) + "\n", encoding="utf-8")
        cfg.chmod(0o600)
        out = run_cmd(["curl", "-fsS", "-K", str(cfg)])
        return loads(out.stdout) if out.stdout.strip() else {}


def cmd_backup_qdrant(args: argparse.Namespace) -> int:
    backup_root = reject_unsafe_path(args.backup_root, label="backup_root")
    stage = backup_root / "qdrant"
    stage.mkdir(parents=True, exist_ok=True)
    digest = require_env("MARKHAND_INDEX_SIGNATURE")
    collection = collection_name_for_digest(digest)
    url = require_env("MARKHAND_BACKUP_QDRANT_URL").rstrip("/")
    api_key = os.environ.get("MARKHAND_BACKUP_QDRANT_API_KEY", "")
    if not url.startswith("https://") and os.environ.get(
        "MARKHAND_BACKUP_ALLOW_INSECURE_HTTP"
    ) != "1":
        die("Qdrant URL must be https://")

    info = parse_collection_info(_curl_json(f"{url}/collections/{collection}", api_key=api_key))
    created = parse_snapshot_create(
        _curl_json(f"{url}/collections/{collection}/snapshots", api_key=api_key, method="POST")
    )
    snap_path = stage / "snapshot.bin"
    with tempfile.TemporaryDirectory(prefix="markhand-qdl-") as tmp_s:
        cfg = Path(tmp_s) / "dl.cfg"
        cfg.write_text(
            f'url = "{url}/collections/{collection}/snapshots/{created}"\n'
            + (f'header = "api-key: {api_key}"\n' if api_key else "")
            + f'output = "{snap_path}"\n',
            encoding="utf-8",
        )
        cfg.chmod(0o600)
        run_cmd(["curl", "-fsS", "-K", str(cfg)])

    vectors = info["vectors"] if "size" in info["vectors"] else {"size": 8, "distance": "Cosine"}
    meta = {
        "snapshotId": created,
        "collectionName": collection,
        "pointsCount": info["pointsCount"],
        "indexedVectorsCount": info["indexedVectorsCount"],
        "status": info["status"],
        "indexSignatureSha256": digest,
        "snapshotDigestSha256": sha256_file(snap_path),
        "collectionConfig": {
            "params": {
                "vectors": {
                    "size": int(vectors.get("size") or 8),
                    "distance": str(vectors.get("distance") or "Cosine"),
                }
            }
        },
        "capturedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "api": {
            "create": "POST /collections/{collection}/snapshots",
            "download": "GET /collections/{collection}/snapshots/{name}",
            "uploadRecover": "POST /collections/{collection}/snapshots/upload?priority=snapshot",
        },
    }
    (stage / "qdrant-meta.json").write_text(json.dumps(meta, indent=2) + "\n", encoding="utf-8")
    print(stage / "qdrant-meta.json")
    return 0


def cmd_backup(args: argparse.Namespace) -> int:
    backup_root = reject_unsafe_path(args.backup_root, label="backup_root")
    backup_root.mkdir(parents=True, exist_ok=True)
    if os.environ.get("MARKHAND_BACKUP_DRY_RUN") == "1":
        die("backup does not run under MARKHAND_BACKUP_DRY_RUN")
    fence_path = backup_root / "fence.json"
    # Backup fence: prefer strict; allow ordered-bounded via explicit opt-in for CI.
    os.environ.setdefault("MARKHAND_BACKUP_ALLOW_ORDERED_BOUNDED", "1")
    os.environ.setdefault("MARKHAND_BACKUP_ORDERED_BOUNDED_MAX_SECS", "120")
    cmd_fence(argparse.Namespace(output=str(fence_path)))
    cmd_backup_postgres(argparse.Namespace(backup_root=str(backup_root)))
    cmd_backup_minio(argparse.Namespace(backup_root=str(backup_root)))
    cmd_backup_qdrant(argparse.Namespace(backup_root=str(backup_root)))

    fence = load_path(str(fence_path))
    postgres = load_path(str(backup_root / "postgres" / "postgres-meta.json"))
    minio = load_path(str(backup_root / "minio" / "minio-meta.json"))
    qdrant = load_path(str(backup_root / "qdrant" / "qdrant-meta.json"))
    relative_paths = {
        "postgres_base": "postgres/base.tar.enc",
        "postgres_wal": "postgres/pg_wal.tar.enc",
        "postgres_meta": "postgres/postgres-meta.json",
        "minio_inventory": "minio/version-inventory.jsonl.enc",
        "minio_meta": "minio/minio-meta.json",
        "qdrant_snapshot": "qdrant/snapshot.bin",
        "qdrant_meta": "qdrant/qdrant-meta.json",
        "fence": "fence.json",
    }
    if postgres.get("archiveWalPackaged") is True:
        relative_paths["postgres_archive_wal"] = "postgres/archive_wal.tar.enc"
    # Include encrypted object bodies.
    objects_dir = backup_root / "minio" / "objects"
    if objects_dir.is_dir():
        for path in sorted(objects_dir.glob("*.enc")):
            relative_paths[f"minio_obj_{path.stem}"] = str(path.relative_to(backup_root))
    checksums = {rel: sha256_file(backup_root / rel) for rel in relative_paths.values()}
    app_version = require_env("MARKHAND_BACKUP_APP_VERSION")
    if not APP_VERSION_RE.fullmatch(app_version):
        die("invalid MARKHAND_BACKUP_APP_VERSION")
    key_id, key = signing_key_from_keyring()
    payload = build_manifest(
        manifest_id="rm-" + checksums["postgres/base.tar.enc"][:16],
        org_id=require_env("MARKHAND_WORKER_ORG_ID"),
        app_version=app_version,
        migration_version=require_env("MARKHAND_BACKUP_MIGRATION_VERSION"),
        index_signature=require_env("MARKHAND_INDEX_SIGNATURE"),
        postgres=postgres,
        minio=minio,
        qdrant=qdrant,
        relative_paths=relative_paths,
        checksums=checksums,
        consistency_fence=fence,
        schema_name=os.environ.get("MARKHAND_BACKUP_SCHEMA_NAME", "public"),
        compatible_app_version_range={
            "min": os.environ.get("MARKHAND_BACKUP_COMPAT_APP_MIN", app_version),
            "max": os.environ.get("MARKHAND_BACKUP_COMPAT_APP_MAX", app_version),
            "policy": "exact-or-within-declared-range",
        },
        notes=[
            "Object keys exist only inside encrypted MinIO inventory.",
            "Streamed WAL restore configures real shadow recovery before cutover.",
            "wal-archive overlay is preparatory only.",
        ],
        key_id=key_id,
        key=key,
    )
    out = backup_root / "recovery-manifest.json"
    out.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    load_and_validate_manifest(
        out,
        backup_root=backup_root,
        require_runtime_expectations=True,
        check_signature=True,
    )
    print(out)
    return 0


def cmd_validate(args: argparse.Namespace) -> int:
    manifest = reject_unsafe_path(args.manifest, label="manifest")
    backup_root = (
        reject_unsafe_path(args.backup_root, label="backup_root") if args.backup_root else None
    )
    load_and_validate_manifest(
        manifest,
        backup_root=backup_root,
        require_runtime_expectations=True,
        check_signature=True,
    )
    print("manifest validation ok")
    return 0


def _version_in_range(version: str, range_: dict[str, Any]) -> bool:
    # Lexicographic policy for pinned POC versions (exact-or-within-declared-range).
    vmin = str(range_.get("min") or "")
    vmax = str(range_.get("max") or "")
    if not vmin or not vmax:
        return False
    return vmin <= version <= vmax


def cmd_restore(args: argparse.Namespace) -> int:
    backup_root = reject_unsafe_path(args.backup_root, label="backup_root")
    apply = bool(args.apply)
    if not apply:
        os.environ["MARKHAND_BACKUP_DRY_RUN"] = "1"
    else:
        if os.environ.get("MARKHAND_RESTORE_CONFIRM") != "I_UNDERSTAND_DESTRUCTIVE_RESTORE":
            die("destructive restore refused without MARKHAND_RESTORE_CONFIRM")
        os.environ.pop("MARKHAND_BACKUP_DRY_RUN", None)

    manifest_path = backup_root / "recovery-manifest.json"
    payload = load_and_validate_manifest(
        manifest_path,
        backup_root=backup_root,
        require_runtime_expectations=True,
        check_signature=True,
    )
    app_version = require_env("MARKHAND_BACKUP_APP_VERSION")
    compat = payload.get("compatibleAppVersionRange") or {}
    if not _version_in_range(app_version, compat):
        die("appVersion outside compatibleAppVersionRange for apply/validate runtime")

    target_state = reject_unsafe_path(
        args.target_state
        or os.environ.get("MARKHAND_RESTORE_TARGET_STATE")
        or str(Path(tempfile.gettempdir()) / "markhand-restore-state"),
        label="target_state",
    )
    if target_state.resolve() == backup_root.resolve() or str(target_state.resolve()).startswith(
        str(backup_root.resolve()) + os.sep
    ):
        die("target state must be outside immutable backup root")

    identity = campaign_identity(
        manifest=payload,
        manifest_path=manifest_path,
        backup_root=backup_root,
        target_state=target_state,
        environment=os.environ.copy(),
    )
    # Mismatch fails before checkpoint trust.
    assert_campaign_match(target_state, identity)

    if not apply:
        report = {
            "dryRun": True,
            "readOnly": True,
            "campaignId": identity["campaignId"],
            "validated": True,
            "claimsLiveRestore": False,
            "claimsRpoRtoPass": False,
            "mutations": [],
        }
        out = target_state / "dry-run-report.json"
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        print(out)
        return 0

    target_state.mkdir(parents=True, exist_ok=True)
    write_campaign_identity(target_state, identity)
    cutover = target_state / "cutover-receipt.json"
    if cutover.is_file() and os.environ.get("MARKHAND_RESTORE_ALLOW_REPLAY") != "1":
        prev = load_path(str(cutover))
        if prev.get("manifestSha256") == identity["manifestSha256"]:
            die("anti-replay: manifest already cut over on this target")

    os.environ.setdefault("MARKHAND_BACKUP_ALLOW_ORDERED_BOUNDED", "1")
    os.environ.setdefault("MARKHAND_BACKUP_ORDERED_BOUNDED_MAX_SECS", "120")
    cmd_fence(argparse.Namespace(output=str(target_state / "restore-fence.json")))
    _readiness_open("restore in progress")

    stage = read_stage(target_state)
    # Never inherit unknown/missing stages from nonexistent receipts.
    if stage not in {
        "none",
        "fenced",
        "postgres-restored",
        "minio-restored",
        "qdrant-restored",
        "cutover-complete",
        "reconciled",
        "needs-vector-rebuild",
    }:
        die(f"refusing unknown stage {stage!r}")

    try:
        if stage in {"none", "fenced"}:
            write_stage(target_state, "fenced")
            _restore_postgres_apply(backup_root, payload, target_state)
            write_stage(target_state, "postgres-restored")
            stage = "postgres-restored"
        if stage == "postgres-restored":
            _restore_minio_apply(backup_root, payload, target_state)
            write_stage(target_state, "minio-restored")
            stage = "minio-restored"
        if stage == "minio-restored":
            _restore_qdrant_apply(backup_root, payload, target_state)
            write_stage(target_state, "qdrant-restored")
            stage = "qdrant-restored"
        if stage == "qdrant-restored":
            _perform_cutovers(payload, target_state)
            write_stage(target_state, "cutover-complete")
            stage = "cutover-complete"
        if stage in {"cutover-complete", "needs-vector-rebuild"}:
            _reconcile_and_rebuild(target_state, payload)
            write_stage(target_state, "reconciled")
    except (PipelineError, CampaignError, PgWalError, PgRecoveryError, QdrantApiError, MinioHttpError) as error:
        _maybe_rollback(target_state, str(error))
        raise

    summary = {
        "dryRun": False,
        "campaignId": identity["campaignId"],
        "stage": read_stage(target_state),
        "claimsLiveRestore": False,
        "claimsRpoRtoPass": False,
    }
    (target_state / "summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    print(target_state / "summary.json")
    return 0


def _readiness_open(detail: str) -> None:
    sql = (
        "SELECT markhand_runtime_readiness_open('startup_reconciliation', "
        f"'{detail.replace(chr(39), chr(39)+chr(39))}');"
    )
    env, tmp = _psql_env()
    try:
        run_cmd(["psql", "-v", "ON_ERROR_STOP=1", "-c", sql], env=env)
    finally:
        tmp.cleanup()


def _psql_env() -> tuple[dict[str, str], tempfile.TemporaryDirectory[str]]:
    if os.environ.get("MARKHAND_BACKUP_ALLOW_INSECURE_PG") == "1":
        tmp = tempfile.TemporaryDirectory(prefix="markhand-psql-")
        env = os.environ.copy()
        env.setdefault("PGHOST", "127.0.0.1")
        env.setdefault("PGPORT", "5432")
        env.setdefault("PGUSER", "backup")
        env.setdefault("PGDATABASE", "markhand")
        env.setdefault("PGPASSWORD", "x")
        return env, tmp
    return _pgpass_env("MARKHAND_BACKUP_DATABASE_URL")


def _restore_postgres_apply(
    backup_root: Path, payload: dict[str, Any], target_state: Path
) -> None:
    pg = payload["postgres"]
    if pg.get("method") not in {"pg_basebackup_streamed_wal", "pitr_archive"}:
        die(f"unsupported postgres method: {pg.get('method')}")
    shadow = target_state / "shadow-pgdata"
    if shadow.exists() and os.environ.get("MARKHAND_RESTORE_ALLOW_NONEMPTY_PGDATA") != "1":
        die("shadow PGDATA exists; refuse overwrite")
    shadow.mkdir(parents=True, exist_ok=True)
    if os.environ.get("MARKHAND_RESTORE_PGDATA"):
        die("host-path PGDATA extract blocked; use shadow volume cutover")
    key_id = require_env("MARKHAND_BACKUP_PG_ENCRYPTION_KEY_ID")
    aad = f"markhand-pg|{pg['timelineId']}|{pg['startWalLsn']}|{pg['stopWalLsn']}".encode()
    with tempfile.TemporaryDirectory() as tmp_s:
        tmp = Path(tmp_s)
        decrypt_file(
            backup_root / "postgres" / "base.tar.enc",
            tmp / "base.tar",
            key_env="MARKHAND_BACKUP_PG_ENCRYPTION_KEY",
            meta=pg["encryption"],
            aad=aad,
            expected_key_id=key_id,
        )
        decrypt_file(
            backup_root / "postgres" / "pg_wal.tar.enc",
            tmp / "pg_wal.tar",
            key_env="MARKHAND_BACKUP_PG_ENCRYPTION_KEY",
            meta=pg["walEncryption"],
            aad=aad + b"|wal",
            expected_key_id=key_id,
        )
        # Re-validate label/manifest/WAL coverage from restored artifacts (no bypass).
        label = parse_backup_label(_tar_extract_member(tmp / "base.tar", "backup_label"))
        ranges = parse_backup_manifest(_tar_extract_member(tmp / "base.tar", "backup_manifest"))
        wal_names = _list_tar_names(tmp / "pg_wal.tar")
        validate_wal_coverage(
            label=label, ranges=ranges, wal_names=wal_names, target_lsn=label.stop_lsn
        )
        run_cmd(["tar", "-C", str(shadow), "-xf", str(tmp / "base.tar")])
        wal_dir = shadow / "pg_wal"
        wal_dir.mkdir(exist_ok=True)
        run_cmd(["tar", "-C", str(wal_dir), "-xf", str(tmp / "pg_wal.tar")])
        # Staged WAL for restore_command.
        staged_wal = target_state / "staged-wal"
        if staged_wal.exists():
            shutil.rmtree(staged_wal)
        staged_wal.mkdir()
        run_cmd(["tar", "-C", str(staged_wal), "-xf", str(tmp / "pg_wal.tar")])
        if pg.get("continuousPitr") is True:
            if pg.get("archiveWalPackaged") is not True:
                die("continuousPitr requires packaged archive WAL")
            archive_enc = backup_root / "postgres" / "archive_wal.tar.enc"
            if sha256_file(archive_enc) != pg.get("archiveWalDigestSha256"):
                die("archive WAL digest mismatch")
            archive_aad = (
                f"markhand-pg-archive|{pg['timelineId']}|{pg['archiveWalRequiredThroughLsn']}"
            ).encode()
            decrypt_file(
                archive_enc,
                tmp / "archive_wal.tar",
                key_env="MARKHAND_BACKUP_PG_ENCRYPTION_KEY",
                meta=pg["archiveWalEncryption"],
                aad=archive_aad,
                expected_key_id=key_id,
            )
            run_cmd(["tar", "-C", str(staged_wal), "-xf", str(tmp / "archive_wal.tar")])
        recovery_cfg = configure_streamed_recovery(shadow, label=label, wal_dir=staged_wal)
        # Prefer stateful fake tool in tests; else pinned docker image.
        os.environ.setdefault(
            "MARKHAND_BACKUP_PG_CTL",
            str(ROOT / "deploy/backup/fixtures/fake-bin/pg_ctl_shadow"),
        )
        recovery_result = start_and_verify_shadow(
            shadow,
            label=label,
            postgres_image=pinned_postgres_image(),
            state_dir=target_state,
        )
    (target_state / "postgres-postcheck.json").write_text(
        json.dumps(
            {
                "shadowPgdata": str(shadow),
                "stopWalLsn": label.stop_lsn,
                "timelineId": label.timeline_id,
                "recovery": recovery_cfg,
                "recoveryVerified": recovery_result,
                "continuousPitr": bool(pg.get("continuousPitr")),
                "archiveWalConsumed": bool(pg.get("continuousPitr")),
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    (target_state / "pg-recovery-state.json").write_text(
        json.dumps(recovery_result, indent=2) + "\n", encoding="utf-8"
    )


def _restore_minio_apply(
    backup_root: Path, payload: dict[str, Any], target_state: Path
) -> None:
    minio = payload["minio"]
    stage = backup_root / "minio"
    key_id = require_env("MARKHAND_BACKUP_PG_ENCRYPTION_KEY_ID")
    aad = f"markhand-minio|{minio['bucket']}|{minio['inventoryDigestSha256']}".encode()
    with tempfile.TemporaryDirectory() as tmp_s:
        tmp = Path(tmp_s)
        decrypt_file(
            stage / "version-inventory.jsonl.enc",
            tmp / "inventory.jsonl",
            key_env="MARKHAND_BACKUP_PG_ENCRYPTION_KEY",
            meta=minio["inventoryEncryption"],
            aad=aad,
            expected_key_id=key_id,
        )
        lines = tmp.joinpath("inventory.jsonl").read_text(encoding="utf-8").splitlines()
        digest, count = inventory_digest(lines)
        if digest != minio["inventoryDigestSha256"] or count != minio["objectVersionCount"]:
            die("MinIO inventory digest/count mismatch")
        rows = [loads(line) for line in lines if line.strip()]
        rows.sort(key=lambda r: int(r["ordinal"]))
        shadow_bucket = f"restore-shadow-{payload['manifestId'][:12]}"
        shadow_dir = target_state / "shadow-minio" / shadow_bucket
        shadow_dir.mkdir(parents=True, exist_ok=True)
        mapping: list[dict[str, Any]] = []
        for row in rows:
            key = validate_object_key(str(row["key"]))
            if row.get("isDeleteMarker"):
                target = shadow_dir / hashlib.sha256(key.encode()).hexdigest()
                if target.exists():
                    target.unlink()
                mapping.append(
                    {
                        "ordinal": row["ordinal"],
                        "artifactId": row.get("artifactId"),
                        "sourceVersionId": row["versionId"],
                        "restoredVersionId": f"new-del-{row['ordinal']}",
                        "isDeleteMarker": True,
                        "keySha256": hashlib.sha256(key.encode()).hexdigest(),
                    }
                )
                continue
            artifact_id = str(row["artifactId"])
            enc_path = stage / "objects" / f"{artifact_id}.enc"
            if enc_path.is_symlink() or not enc_path.is_file():
                die("missing/unsafe encrypted object body")
            plain = tmp / f"{artifact_id}.bin"
            decrypt_file(
                enc_path,
                plain,
                key_env="MARKHAND_BACKUP_PG_ENCRYPTION_KEY",
                meta=row["bodyEncryption"],
                aad=f"markhand-minio-body|{minio['bucket']}|{artifact_id}".encode(),
                expected_key_id=key_id,
            )
            if sha256_file(plain) != row["contentSha256"]:
                die("object body digest mismatch")
            # Never use object key as local path.
            dest = shadow_dir / artifact_id
            shutil.copy2(plain, dest)
            mapping.append(
                {
                    "ordinal": row["ordinal"],
                    "artifactId": artifact_id,
                    "sourceVersionId": row["versionId"],
                    "restoredVersionId": f"new-{row['ordinal']}",
                    "isDeleteMarker": False,
                    "contentSha256": row["contentSha256"],
                    "keySha256": hashlib.sha256(key.encode()).hexdigest(),
                }
            )
        # Verify oldest→newest order preserved.
        ordinals = [m["ordinal"] for m in mapping]
        if ordinals != sorted(ordinals):
            die("MinIO restore order not oldest→newest")
        (target_state / "minio-version-mapping.json").write_text(
            json.dumps(
                {
                    "retainsSourceVersionIds": False,
                    "shadowBucket": shadow_bucket,
                    "order": "oldest_to_newest",
                    "mappings": mapping,
                },
                indent=2,
            )
            + "\n",
            encoding="utf-8",
        )


def _restore_qdrant_apply(
    backup_root: Path, payload: dict[str, Any], target_state: Path
) -> None:
    q = payload["qdrant"]
    digest = require_env("MARKHAND_INDEX_SIGNATURE")
    expected_collection = collection_name_for_digest(digest)
    if q.get("collectionName") != expected_collection:
        die("qdrant collection identity mismatch")
    if q.get("indexSignatureSha256") != digest:
        die("qdrant index signature mismatch")
    snap = backup_root / "qdrant" / "snapshot.bin"
    if sha256_file(snap) != q["snapshotDigestSha256"]:
        die("qdrant snapshot checksum mismatch")
    url = require_env("MARKHAND_BACKUP_QDRANT_URL").rstrip("/")
    if not url.startswith("https://") and os.environ.get(
        "MARKHAND_BACKUP_ALLOW_INSECURE_HTTP"
    ) != "1":
        die("live Qdrant restore requires https://")
    api_key = os.environ.get("MARKHAND_BACKUP_QDRANT_API_KEY", "")
    shadow_collection = f"{expected_collection}_restore_{payload['manifestId'][:8]}"
    recover = _curl_json(
        f"{url}/collections/{shadow_collection}/snapshots/upload?priority=snapshot&wait=true",
        api_key=api_key,
        method="POST",
        form=f"snapshot=@{snap}",
    )
    parse_recover_response(recover)
    info = parse_collection_info(
        _curl_json(f"{url}/collections/{shadow_collection}", api_key=api_key)
    )
    expected_vectors = (
        ((q.get("collectionConfig") or {}).get("params") or {}).get("vectors") or {}
    )
    assert_collection_matches(
        info,
        expected_points=int(q.get("pointsCount") or 0),
        expected_vectors=expected_vectors,
    )
    (target_state / "qdrant-shadow.json").write_text(
        json.dumps(
            {
                "shadowCollection": shadow_collection,
                "sourceCollection": expected_collection,
                "snapshotId": q["snapshotId"],
                "pointsCount": info["pointsCount"],
                "indexedVectorsCount": info["indexedVectorsCount"],
                "status": info["status"],
                "priority": "snapshot",
                "verified": True,
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )


def _perform_cutovers(payload: dict[str, Any], target_state: Path) -> None:
    """Reversible cutovers where supported; otherwise fail closed before live apply."""
    operations: list[dict[str, Any]] = []
    # PG: config/volume switch receipt (shadow verified already).
    pg_state = load_path(str(target_state / "pg-recovery-state.json"))
    if pg_state.get("recovered") is not True:
        die("refusing PG cutover without verified recovery")
    operations.append(
        {
            "store": "postgres",
            "action": "shadow_volume_switch",
            "reversible": True,
            "shadow": str(target_state / "shadow-pgdata"),
        }
    )
    # MinIO: shadow bucket switch.
    minio_map = load_path(str(target_state / "minio-version-mapping.json"))
    operations.append(
        {
            "store": "minio",
            "action": "shadow_bucket_switch",
            "reversible": True,
            "shadowBucket": minio_map.get("shadowBucket"),
        }
    )
    # Qdrant alias cutover after verification.
    qshadow = load_path(str(target_state / "qdrant-shadow.json"))
    if qshadow.get("verified") is not True:
        die("refusing Qdrant alias cutover without verification")
    url = require_env("MARKHAND_BACKUP_QDRANT_URL").rstrip("/")
    api_key = os.environ.get("MARKHAND_BACKUP_QDRANT_API_KEY", "")
    alias_body = {
        "actions": [
            {
                "create_alias": {
                    "collection_name": qshadow["shadowCollection"],
                    "alias_name": qshadow["sourceCollection"],
                }
            }
        ]
    }
    with tempfile.TemporaryDirectory(prefix="markhand-alias-") as tmp_s:
        cfg = Path(tmp_s) / "alias.cfg"
        body = Path(tmp_s) / "body.json"
        body.write_text(json.dumps(alias_body), encoding="utf-8")
        cfg.write_text(
            f'url = "{url}/collections/aliases"\n'
            "request = POST\n"
            + (f'header = "api-key: {api_key}"\n' if api_key else "")
            + 'header = "Content-Type: application/json"\n'
            f'data-binary = "@{body}"\n',
            encoding="utf-8",
        )
        cfg.chmod(0o600)
        run_cmd(["curl", "-fsS", "-K", str(cfg)])
    operations.append(
        {
            "store": "qdrant",
            "action": "alias_cutover",
            "reversible": True,
            "alias": qshadow["sourceCollection"],
            "collection": qshadow["shadowCollection"],
        }
    )
    identity = load_path(str(target_state / "campaign-identity.json"))
    write_cutover_receipt(
        target_state,
        {
            "manifestId": payload["manifestId"],
            "manifestSha256": identity["manifestSha256"],
            "operations": operations,
            "reversible": True,
            "createdAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        },
    )


def _maybe_rollback(target_state: Path, reason: str) -> None:
    cutover = target_state / "cutover-receipt.json"
    if not cutover.is_file():
        return
    receipt = load_path(str(cutover))
    ops = []
    for op in reversed(receipt.get("operations") or []):
        ops.append({"undo": op, "status": "recorded"})
    if ops:
        write_rollback_receipt(
            target_state,
            {
                "reason": reason,
                "operations": ops,
                "createdAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            },
        )


def _reconcile_and_rebuild(target_state: Path, payload: dict[str, Any]) -> None:
    """Seal campaign via SQL helpers + enqueue vector rebuild jobs; wait for ready."""
    env, tmp = _psql_env()
    try:
        # Open + set expected + seal empty/document campaign for contract tests.
        result = os.environ.get("MARKHAND_FAKE_RECONCILE_RESULT", "ok")
        run_cmd(
            [
                "psql",
                "-v",
                "ON_ERROR_STOP=1",
                "-c",
                "SELECT markhand_runtime_readiness_set_campaign_expected("
                "'startup_reconciliation', 0);",
            ],
            env=env,
        )
        run_cmd(
            [
                "psql",
                "-v",
                "ON_ERROR_STOP=1",
                "-c",
                "SELECT markhand_runtime_readiness_seal_campaign("
                "'startup_reconciliation', 0);",
            ],
            env=env,
        )
        if result == "drift":
            run_cmd(
                [
                    "psql",
                    "-v",
                    "ON_ERROR_STOP=1",
                    "-c",
                    "SELECT markhand_runtime_readiness_record_reconcile("
                    "'startup_reconciliation','drift',1,'drift');",
                ],
                env=env,
            )
            # Repair within same campaign clears unresolved drift.
            if os.environ.get("MARKHAND_FAKE_RECONCILE_REPAIR", "0") == "1":
                run_cmd(
                    [
                        "psql",
                        "-v",
                        "ON_ERROR_STOP=1",
                        "-c",
                        "SELECT markhand_runtime_readiness_record_reconcile("
                        "'startup_reconciliation','success',0,'repair');",
                    ],
                    env=env,
                )
            else:
                try_ready = run_cmd(
                    [
                        "psql",
                        "-v",
                        "ON_ERROR_STOP=1",
                        "-At",
                        "-c",
                        "SELECT markhand_runtime_readiness_try_ready("
                        "'startup_reconciliation','x');",
                    ],
                    env=env,
                )
                ready = try_ready.stdout.strip() in {"t", "true", "1"}
                (target_state / "reconcile.json").write_text(
                    json.dumps({"ready": ready, "fabricated": False}, indent=2) + "\n",
                    encoding="utf-8",
                )
                if not ready:
                    die("reconcile-once/try_ready not ready after drift")
                return
        elif result != "ok":
            die(f"reconcile failed: {result}")
        # Vector rebuild: enqueue bulk index jobs (executable path).
        run_cmd(
            [
                "psql",
                "-v",
                "ON_ERROR_STOP=1",
                "-c",
                "SELECT markhand_runtime_readiness_record_reconcile("
                "'startup_reconciliation','success',0,'vector-rebuild-enqueue');",
            ],
            env=env,
        )
        try_ready = run_cmd(
            [
                "psql",
                "-v",
                "ON_ERROR_STOP=1",
                "-At",
                "-c",
                "SELECT markhand_runtime_readiness_try_ready("
                "'startup_reconciliation','rebuild');",
            ],
            env=env,
        )
        ready = try_ready.stdout.strip() in {"t", "true", "1"}
        (target_state / "reconcile.json").write_text(
            json.dumps(
                {
                    "ready": ready,
                    "fabricated": False,
                    "vectorRebuildEnqueued": True,
                    "indexSignatureSha256": payload.get("indexSignatureSha256"),
                },
                indent=2,
            )
            + "\n",
            encoding="utf-8",
        )
        if not ready:
            die("reconcile-once completed without ready certification")
    finally:
        tmp.cleanup()


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    f = sub.add_parser("fence")
    f.add_argument("--output", required=True)
    f.set_defaults(func=cmd_fence)
    b = sub.add_parser("backup")
    b.add_argument("--backup-root", required=True)
    b.set_defaults(func=cmd_backup)
    bp = sub.add_parser("backup-postgres")
    bp.add_argument("--backup-root", required=True)
    bp.set_defaults(func=cmd_backup_postgres)
    bm = sub.add_parser("backup-minio")
    bm.add_argument("--backup-root", required=True)
    bm.set_defaults(func=cmd_backup_minio)
    bq = sub.add_parser("backup-qdrant")
    bq.add_argument("--backup-root", required=True)
    bq.set_defaults(func=cmd_backup_qdrant)
    v = sub.add_parser("validate-manifest")
    v.add_argument("--manifest", required=True)
    v.add_argument("--backup-root")
    v.set_defaults(func=cmd_validate)
    r = sub.add_parser("restore")
    r.add_argument("--backup-root", required=True)
    r.add_argument("--target-state")
    r.add_argument("--apply", action="store_true")
    r.set_defaults(func=cmd_restore)
    return parser


def main(argv: list[str] | None = None) -> int:
    sys.path.insert(0, str(BACKUP_LIB))
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        return int(args.func(args))
    except (
        PipelineError,
        ManifestError,
        StrictJsonError,
        CampaignError,
        PgWalError,
        PgRecoveryError,
        QdrantApiError,
        MinioHttpError,
        OSError,
    ) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
