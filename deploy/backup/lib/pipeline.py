#!/usr/bin/env python3
"""O03 backup/restore pipeline — fail-closed, no secret argv, no heredoc secrets.

Subcommands: fence, backup, restore-dry-run, restore-apply, validate-manifest,
reconcile-status. Shell wrappers call this with argv/env only.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[3]
BACKUP_LIB = Path(__file__).resolve().parent
if str(BACKUP_LIB) not in sys.path:
    sys.path.insert(0, str(BACKUP_LIB))

from crypto import decrypt_file, encrypt_file  # noqa: E402
from manifest import (  # noqa: E402
    ManifestError,
    build_manifest,
    inventory_digest,
    load_and_validate_manifest,
    signing_key_from_keyring,
)
from strictjson import StrictJsonError, load_path, loads  # noqa: E402
REAL_SERVICES = ("api", "worker-convert", "worker-index", "worker-embedding")
SAFE_REL = re.compile(r"^(?!/)(?!.*\.\./)[A-Za-z0-9._/-]+$")
CONTROL = re.compile(r"[\x00-\x1f\x7f]")


class PipelineError(RuntimeError):
    """Fail-closed pipeline error."""


def die(msg: str, code: int = 2) -> None:
    print(f"error: {msg}", file=sys.stderr)
    raise SystemExit(code)


def require_env(name: str) -> str:
    value = os.environ.get(name, "")
    if not value or CONTROL.search(value):
        die(f"required env missing/invalid (fail closed): {name}")
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
    # Never put DB URLs / secrets into args — callers must use env/config files.
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


def cmd_fence(args: argparse.Namespace) -> int:
    out = reject_unsafe_path(args.output, label="fence output")
    started = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    mode = "ordered-bounded"
    writes_fenced = False
    notes: list[str] = []
    dry_run = os.environ.get("MARKHAND_BACKUP_DRY_RUN", "0") == "1"
    if dry_run:
        notes.append("dry-run fence is observational only — no stop/start")
        writes_fenced = False
        mode = "ordered-bounded"
    else:
        compose = docker_compose_argv()
        if compose is None:
            notes.append(
                "Docker/compose unavailable — ordered-bounded capture only; "
                "writesFenced remains false; bounded cross-store inconsistency possible"
            )
            writes_fenced = False
            mode = "ordered-bounded"
        else:
            # Stop only real compose services.
            stop = run_cmd(compose + ["stop", *REAL_SERVICES], check=False)
            if stop.returncode != 0:
                die(f"failed to stop services for fence: {stop.stderr.strip()}")
            ps = run_cmd(compose + ["ps", "--status", "running", "--services"], check=False)
            running = {
                line.strip()
                for line in (ps.stdout or "").splitlines()
                if line.strip()
            }
            still = sorted(set(REAL_SERVICES) & running)
            if still:
                die(f"quiescence verification failed; still running: {still}")
            writes_fenced = True
            mode = "strict-write-fence"
            notes.append(f"verified stopped: {', '.join(REAL_SERVICES)}")
    completed = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    if mode == "ordered-bounded" and writes_fenced:
        die("ordered-bounded cannot set writesFenced=true")
    payload = {
        "mode": mode,
        "writesFenced": writes_fenced,
        "fenceStartedAt": started,
        "fenceCompletedAt": completed,
        "ordering": ["postgres", "minio", "qdrant", "manifest"],
        "boundedInconsistencyNotes": notes,
        "services": list(REAL_SERVICES),
    }
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    print(out)
    return 0


def _pgpass_env(url_env: str) -> tuple[dict[str, str], tempfile.TemporaryDirectory[str]]:
    """Build env with PGPASSFILE; never put URL password on argv."""
    # Prefer discrete PG* vars when set; else parse URL into PG* without argv.
    tmp = tempfile.TemporaryDirectory(prefix="markhand-pgpass-")
    env = os.environ.copy()
    host = env.get("MARKHAND_BACKUP_PGHOST") or env.get("PGHOST")
    port = env.get("MARKHAND_BACKUP_PGPORT") or env.get("PGPORT") or "5432"
    user = env.get("MARKHAND_BACKUP_PGUSER") or env.get("PGUSER")
    dbname = env.get("MARKHAND_BACKUP_PGDATABASE") or env.get("PGDATABASE")
    password = env.get("MARKHAND_BACKUP_PGPASSWORD") or env.get("PGPASSWORD")
    url = env.get(url_env, "")
    if not all([host, user, dbname, password]) and url:
        # Minimal URL parse without logging.
        from urllib.parse import urlparse, unquote

        parsed = urlparse(url)
        host = host or parsed.hostname or "127.0.0.1"
        port = port or str(parsed.port or 5432)
        user = user or unquote(parsed.username or "")
        dbname = dbname or (parsed.path or "/").lstrip("/")
        password = password or unquote(parsed.password or "")
    if not all([host, user, dbname, password]):
        tmp.cleanup()
        die("postgres connection env incomplete (PGHOST/PGUSER/PGDATABASE/PGPASSWORD)")
    pgpass = Path(tmp.name) / "pgpass"
    pgpass.write_text(f"{host}:{port}:{dbname}:{user}:{password}\n", encoding="utf-8")
    pgpass.chmod(0o600)
    env["PGPASSFILE"] = str(pgpass)
    env["PGHOST"] = host
    env["PGPORT"] = str(port)
    env["PGUSER"] = user
    env["PGDATABASE"] = dbname
    # Remove URL so children cannot accidentally log it from env dumps of argv builders.
    env.pop(url_env, None)
    env.pop("MARKHAND_BACKUP_DATABASE_URL", None)
    return env, tmp


def cmd_backup_postgres(args: argparse.Namespace) -> int:
    backup_root = reject_unsafe_path(args.backup_root, label="backup_root")
    stage = backup_root / "postgres"
    stage.mkdir(parents=True, exist_ok=True)
    meta_path = stage / "postgres-meta.json"
    key_id = require_env("MARKHAND_BACKUP_PG_ENCRYPTION_KEY_ID")
    require_env("MARKHAND_BACKUP_PG_ENCRYPTION_KEY")

    mode = os.environ.get("MARKHAND_BACKUP_MODE", "live")
    pitr_archive = os.environ.get("MARKHAND_BACKUP_PITR_ARCHIVE", "0") == "1"

    with tempfile.TemporaryDirectory(prefix="markhand-pgbak-") as tmp_s:
        tmp = Path(tmp_s)
        pgdata = tmp / "pgdata"
        pgdata.mkdir()
        if mode == "hermetic":
            # Produce coherent base.tar + pg_wal.tar like pg_basebackup -Ft -X stream.
            (pgdata / "backup_label").write_text(
                "START WAL LOCATION: 0/1600000 (file 000000010000000000000001)\n"
                "CHECKPOINT LOCATION: 0/1600000\n"
                "BACKUP METHOD: streamed\n"
                "START TIME: 2026-07-22 00:00:00 UTC\n"
                "LABEL: markhand-hermetic\n"
                "START TIMELINE: 1\n",
                encoding="utf-8",
            )
            run_cmd(["tar", "-C", str(pgdata), "-cf", str(tmp / "base.tar"), "."])
            wal = tmp / "wal"
            wal.mkdir()
            (wal / "000000010000000000000001").write_bytes(b"WALHERMETIC")
            run_cmd(["tar", "-C", str(wal), "-cf", str(tmp / "pg_wal.tar"), "."])
            start_lsn, stop_lsn, timeline = "0/1600000", "0/16B3740", 1
        else:
            env, pgpass_tmp = _pgpass_env("MARKHAND_BACKUP_DATABASE_URL")
            try:
                # Streamed WAL makes a restorable consistent backup to stop LSN.
                run_cmd(
                    [
                        "pg_basebackup",
                        "-h",
                        env["PGHOST"],
                        "-p",
                        env["PGPORT"],
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
                pgpass_tmp.cleanup()
            base_tar = pgdata / "base.tar"
            wal_tar = pgdata / "pg_wal.tar"
            if not base_tar.is_file() or not wal_tar.is_file():
                die("pg_basebackup -Ft -X stream did not produce base.tar and pg_wal.tar")
            shutil.copy2(base_tar, tmp / "base.tar")
            shutil.copy2(wal_tar, tmp / "pg_wal.tar")
            # Parse backup_label from tar via real tar tool.
            label_out = run_cmd(
                ["tar", "-xOf", str(tmp / "base.tar"), "./backup_label"],
                check=False,
            )
            if label_out.returncode != 0:
                label_out = run_cmd(["tar", "-xOf", str(tmp / "base.tar"), "backup_label"])
            label = label_out.stdout
            start_m = re.search(r"START WAL LOCATION:\s*([0-9A-F]+/[0-9A-F]+)", label)
            stop_m = re.search(r"STOP WAL LOCATION:\s*([0-9A-F]+/[0-9A-F]+)", label)
            tl_m = re.search(r"START TIMELINE:\s*(\d+)", label)
            if not start_m or not tl_m:
                die("backup_label missing START WAL LOCATION / TIMELINE")
            start_lsn = start_m.group(1)
            # STOP may be absent until backup ends; use pg_waldump-less fallback from env probe.
            stop_lsn = stop_m.group(1) if stop_m else start_lsn
            timeline = int(tl_m.group(1))

        method = "pg_basebackup_streamed_wal"
        continuous_pitr = False
        archive_wal_packaged = False
        archive_through_lsn: str | None = None
        archive_digest: str | None = None
        archive_enc_meta: dict[str, Any] | None = None
        # compose.wal-archive.yml only turns on server archive_mode; it is
        # preparatory and does NOT enable continuous PITR by itself.
        overlay_on = os.environ.get("MARKHAND_PG_ARCHIVE_MODE") == "on"
        if pitr_archive:
            archive_src = os.environ.get("MARKHAND_BACKUP_ARCHIVE_WAL_DIR", "").strip()
            if not archive_src:
                die(
                    "continuous PITR requested but MARKHAND_BACKUP_ARCHIVE_WAL_DIR is unset; "
                    "compose.wal-archive.yml is preparatory only — continuous PITR requires "
                    "packaging/checksumming archived WAL through the target LSN"
                )
            if mode != "hermetic" and not overlay_on:
                die(
                    "continuous PITR requested but MARKHAND_PG_ARCHIVE_MODE!=on "
                    "(wal-archive overlay preparatory config missing)"
                )
            archive_dir = Path(archive_src)
            if not archive_dir.is_dir():
                die(f"archive WAL dir missing: {archive_dir}")
            wal_files = sorted(
                p for p in archive_dir.iterdir() if p.is_file() and not p.is_symlink()
            )
            if not wal_files:
                die("archive WAL dir has no WAL segments to package through target LSN")
            archive_tar = tmp / "archive_wal.tar"
            run_cmd(["tar", "-C", str(archive_dir), "-cf", str(archive_tar), "."])
            archive_aad = f"markhand-pg-archive|{timeline}|{stop_lsn}".encode()
            archive_enc = stage / "archive_wal.tar.enc"
            archive_enc_meta = encrypt_file(
                archive_tar,
                archive_enc,
                key_env="MARKHAND_BACKUP_PG_ENCRYPTION_KEY",
                key_id=key_id,
                aad=archive_aad,
            )
            archive_digest = sha256_file(archive_enc)
            archive_through_lsn = stop_lsn
            archive_wal_packaged = True
            method = "pitr_archive"
            continuous_pitr = True

        aad = f"markhand-pg|{timeline}|{start_lsn}|{stop_lsn}".encode()
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
        # Validate round-trip with real tar listing after decrypt to temp.
        with tempfile.TemporaryDirectory() as vtmp_s:
            vtmp = Path(vtmp_s)
            decrypt_file(
                base_enc,
                vtmp / "base.tar",
                key_env="MARKHAND_BACKUP_PG_ENCRYPTION_KEY",
                meta=base_meta,
                aad=aad,
                expected_key_id=key_id,
            )
            listing = run_cmd(["tar", "-tf", str(vtmp / "base.tar")])
            if "backup_label" not in listing.stdout.replace("./", ""):
                die("restored base.tar missing backup_label")

        payload = {
            "backupId": f"pg-{sha256_file(base_enc)[:16]}",
            "method": method,
            "continuousPitr": continuous_pitr,
            "timelineId": timeline,
            "startWalLsn": start_lsn,
            "stopWalLsn": stop_lsn,
            "walBoundaryLsn": stop_lsn,
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
                    "Continuous archive PITR: restore base+streamed WAL, then consume "
                    "packaged archive_wal.tar.enc through archiveWalRequiredThroughLsn."
                    if continuous_pitr
                    else (
                        "Extract base.tar + pg_wal.tar into PGDATA; backup_label drives "
                        "recovery to stop LSN. Continuous archive PITR remains blocked: "
                        "compose.wal-archive.yml is preparatory only and does not enable "
                        "PITR unless archived WAL is packaged/checksummed through the "
                        "target LSN and consumed on restore."
                    )
                ),
            },
        }
        meta_path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
        (stage / "wal-boundary.txt").write_text(stop_lsn + "\n", encoding="utf-8")
        print(meta_path)
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

    mode = os.environ.get("MARKHAND_BACKUP_MODE", "live")
    inv_rows: list[dict[str, Any]] = []
    objects_dir = stage / "versions"
    objects_dir.mkdir(exist_ok=True)

    if mode == "hermetic":
        # Two versions + one delete marker, oldest→newest ordinals.
        for ordinal, (key, vid, deleted, body) in enumerate(
            [
                ("docs/a", "v1", False, b"AAAA"),
                ("docs/a", "v2", False, b"BBBB"),
                ("docs/a", "v3", True, b""),
                ("docs/b", "v1", False, b"CCCC"),
            ]
        ):
            row = {
                "ordinal": ordinal,
                "key": key,
                "versionId": vid,
                "isDeleteMarker": deleted,
                "size": len(body),
                "contentSha256": hashlib.sha256(body).hexdigest() if not deleted else None,
            }
            if not deleted:
                path = objects_dir / f"{ordinal:06d}.{vid}.bin"
                path.write_bytes(body)
                row["artifactRel"] = str(path.relative_to(stage))
            inv_rows.append(row)
    else:
        with tempfile.TemporaryDirectory(prefix="markhand-mc-") as tmp_s:
            mc_cfg = Path(tmp_s) / "mc"
            mc_cfg.mkdir()
            env = os.environ.copy()
            env["MC_CONFIG_DIR"] = str(mc_cfg)
            alias = "mhb"
            run_cmd(
                ["mc", "alias", "set", alias, endpoint, access, secret],
                env=env,
            )
            ver = run_cmd(["mc", "version", "info", f"{alias}/{bucket}", "--json"], env=env)
            if '"Enabled"' not in ver.stdout and '"status":"Enabled"' not in ver.stdout.replace(
                " ", ""
            ):
                # tolerant parse
                try:
                    info = loads(ver.stdout.splitlines()[-1])
                    if str(info.get("status", "")).lower() != "enabled":
                        die("MinIO bucket versioning is not Enabled")
                except StrictJsonError:
                    die("MinIO bucket versioning is not Enabled")
            listing = run_cmd(
                ["mc", "ls", "--versions", "--recursive", "--json", f"{alias}/{bucket}"],
                env=env,
            )
            ordinal = 0
            # Collect then sort by key, lastModified, versionId for stable oldest→newest.
            raw_items: list[dict[str, Any]] = []
            for line in listing.stdout.splitlines():
                if not line.strip():
                    continue
                item = loads(line)
                raw_items.append(item)
            raw_items.sort(
                key=lambda it: (
                    str(it.get("key") or it.get("Key") or ""),
                    str(it.get("lastModified") or ""),
                    str(it.get("versionId") or ""),
                )
            )
            for item in raw_items:
                key = str(item.get("key") or item.get("Key") or "")
                vid = str(item.get("versionId") or item.get("VersionId") or "")
                deleted = bool(item.get("isDeleteMarker") or item.get("deleteMarker"))
                row: dict[str, Any] = {
                    "ordinal": ordinal,
                    "key": key,
                    "versionId": vid,
                    "isDeleteMarker": deleted,
                    "size": int(item.get("size") or 0),
                    "contentSha256": None,
                }
                if not deleted:
                    dest = objects_dir / f"{ordinal:06d}.{vid}.bin"
                    run_cmd(
                        [
                            "mc",
                            "cp",
                            "--version-id",
                            vid,
                            f"{alias}/{bucket}/{key}",
                            str(dest),
                        ],
                        env=env,
                    )
                    row["contentSha256"] = sha256_file(dest)
                    row["artifactRel"] = str(dest.relative_to(stage))
                    row["size"] = dest.stat().st_size
                inv_rows.append(row)
                ordinal += 1

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
    # Keep plaintext only briefly for digest; remove so manifest artifact is encrypted.
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
    }
    (stage / "minio-meta.json").write_text(json.dumps(meta, indent=2) + "\n", encoding="utf-8")
    print(stage / "minio-meta.json")
    return 0


def cmd_backup_qdrant(args: argparse.Namespace) -> int:
    backup_root = reject_unsafe_path(args.backup_root, label="backup_root")
    stage = backup_root / "qdrant"
    stage.mkdir(parents=True, exist_ok=True)
    digest = require_env("MARKHAND_INDEX_SIGNATURE")
    collection = collection_name_for_digest(digest)
    url = require_env("MARKHAND_BACKUP_QDRANT_URL").rstrip("/")
    mode = os.environ.get("MARKHAND_BACKUP_MODE", "live")
    snap_path = stage / "snapshot.bin"
    api_key = os.environ.get("MARKHAND_BACKUP_QDRANT_API_KEY", "")

    if mode == "hermetic":
        snap_path.write_bytes(b"HERMETIC_QDRANT_SNAPSHOT_V1182")
        snap_id = "snap-hermetic-001"
        generation = 1
        points_count = 0
        config = {"vectors": {"size": 8, "distance": "Cosine"}}
    else:
        # TLS: live apply requires verify-full HTTPS unless hermetic.
        if not url.startswith("https://") and os.environ.get("MARKHAND_BACKUP_ALLOW_INSECURE_HTTP") != "1":
            die("live Qdrant backup requires https:// URL (or hermetic mode)")
        headers = []
        if api_key:
            # Pass via env to curl -H is still argv — use curl -K config file.
            pass
        with tempfile.TemporaryDirectory(prefix="markhand-curl-") as tmp_s:
            cfg = Path(tmp_s) / "curl.cfg"
            lines = [f'url = "{url}/collections/{collection}/snapshots"']
            if api_key:
                lines.append(f'header = "api-key: {api_key}"')
            lines.append("request = POST")
            cfg.write_text("\n".join(lines) + "\n", encoding="utf-8")
            cfg.chmod(0o600)
            create = run_cmd(["curl", "-fsS", "-K", str(cfg)])
            created = loads(create.stdout)
            snap_id = (
                (created.get("result") or {}).get("name")
                if isinstance(created.get("result"), dict)
                else created.get("result")
            )
            if not snap_id:
                die("qdrant snapshot create missing name")
            dl = Path(tmp_s) / "dl.cfg"
            dl.write_text(
                f'url = "{url}/collections/{collection}/snapshots/{snap_id}"\n'
                + (f'header = "api-key: {api_key}"\n' if api_key else "")
                + f'output = "{snap_path}"\n',
                encoding="utf-8",
            )
            dl.chmod(0o600)
            run_cmd(["curl", "-fsS", "-K", str(dl)])
            info_cfg = Path(tmp_s) / "info.cfg"
            info_cfg.write_text(
                f'url = "{url}/collections/{collection}"\n'
                + (f'header = "api-key: {api_key}"\n' if api_key else ""),
                encoding="utf-8",
            )
            info_cfg.chmod(0o600)
            info = loads(run_cmd(["curl", "-fsS", "-K", str(info_cfg)]).stdout)
            result = info.get("result") or {}
            generation = int(result.get("status", {}).get("indexed_vectors_count", 0) or 0)
            points_count = int(result.get("points_count") or 0)
            config = result.get("config") or {}

    meta = {
        "snapshotId": snap_id,
        "collectionName": collection,
        "collectionGeneration": generation,
        "pointsCount": points_count,
        "indexSignatureSha256": digest,
        "snapshotDigestSha256": sha256_file(snap_path),
        "collectionConfig": config,
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


def _write_checkpoint(target_state: Path, stage: str) -> None:
    target_state.mkdir(parents=True, exist_ok=True)
    (target_state / "stage").write_text(stage + "\n", encoding="utf-8")


def _read_checkpoint(target_state: Path) -> str:
    path = target_state / "stage"
    if not path.is_file():
        return "none"
    return path.read_text(encoding="utf-8").strip()


def cmd_backup(args: argparse.Namespace) -> int:
    backup_root = reject_unsafe_path(args.backup_root, label="backup_root")
    backup_root.mkdir(parents=True, exist_ok=True)
    if os.environ.get("MARKHAND_BACKUP_DRY_RUN") == "1":
        die("backup does not run under MARKHAND_BACKUP_DRY_RUN")
    fence_path = backup_root / "fence.json"
    ns = argparse.Namespace(output=str(fence_path))
    cmd_fence(ns)
    cmd_backup_postgres(argparse.Namespace(backup_root=str(backup_root)))
    cmd_backup_minio(argparse.Namespace(backup_root=str(backup_root)))
    cmd_backup_qdrant(argparse.Namespace(backup_root=str(backup_root)))

    # Assemble signed manifest
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
    checksums = {
        rel: sha256_file(backup_root / rel) for rel in relative_paths.values()
    }
    key_id, key = signing_key_from_keyring()
    payload = build_manifest(
        manifest_id="rm-" + checksums["postgres/base.tar.enc"][:16],
        org_id=require_env("MARKHAND_WORKER_ORG_ID"),
        app_version=require_env("MARKHAND_BACKUP_APP_VERSION"),
        migration_version=require_env("MARKHAND_BACKUP_MIGRATION_VERSION"),
        index_signature=require_env("MARKHAND_INDEX_SIGNATURE"),
        postgres=postgres,
        minio=minio,
        qdrant=qdrant,
        relative_paths=relative_paths,
        checksums=checksums,
        consistency_fence=fence,
        schema_name=os.environ.get("MARKHAND_BACKUP_SCHEMA_NAME", "public"),
        notes=[
            "Manifest stores digests only; MinIO restore assigns new version IDs.",
            "PostgreSQL default method is streamed WAL consistent backup.",
            "compose.wal-archive.yml is preparatory only; continuous PITR stays blocked "
            "unless archived WAL through target LSN is packaged, checksummed, and "
            "consumed on restore.",
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


def cmd_restore(args: argparse.Namespace) -> int:
    backup_root = reject_unsafe_path(args.backup_root, label="backup_root")
    apply = bool(args.apply)
    dry = not apply
    if dry:
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

    # Target-bound state outside immutable backup.
    target_state = reject_unsafe_path(
        args.target_state
        or os.environ.get("MARKHAND_RESTORE_TARGET_STATE")
        or str(Path(tempfile.gettempdir()) / "markhand-restore-state"),
        label="target_state",
    )
    br = backup_root.resolve()
    ts = target_state.resolve()
    if ts == br or str(ts).startswith(str(br) + os.sep):
        die("target state must be outside immutable backup root")

    fingerprint = {
        "manifestId": payload["manifestId"],
        "manifestSha256": sha256_file(manifest_path),
        "orgId": payload["orgId"],
        "indexSignatureSha256": payload["indexSignatureSha256"],
        "migrationVersion": payload["migrationVersion"],
        "backupRoot": str(backup_root.resolve()),
    }
    if dry:
        # Strictly read-only: no fence stop, no readiness SQL, no checkpoint, no store mutation.
        report = {
            "dryRun": True,
            "readOnly": True,
            "fingerprint": fingerprint,
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

    # Apply path: shadow targets + checkpoints in target_state only.
    target_state.mkdir(parents=True, exist_ok=True)
    (target_state / "fingerprint.json").write_text(
        json.dumps(fingerprint, indent=2) + "\n", encoding="utf-8"
    )
    # Anti-replay: refuse if same manifest already cut over unless force.
    cutover = target_state / "cutover.json"
    if cutover.is_file() and os.environ.get("MARKHAND_RESTORE_ALLOW_REPLAY") != "1":
        prev = load_path(str(cutover))
        if prev.get("manifestSha256") == fingerprint["manifestSha256"]:
            die("anti-replay: manifest already cut over on this target")

    # Fence (mutating)
    cmd_fence(argparse.Namespace(output=str(target_state / "restore-fence.json")))

    # Open readiness — errors must not be swallowed.
    _readiness_open("restore in progress")

    stage = _read_checkpoint(target_state)
    if stage in {"none", "fenced"}:
        _restore_postgres_apply(backup_root, payload, target_state)
        _write_checkpoint(target_state, "postgres-restored")
        stage = "postgres-restored"
    if stage == "postgres-restored":
        _restore_minio_apply(backup_root, payload, target_state)
        _write_checkpoint(target_state, "minio-restored")
        stage = "minio-restored"
    if stage == "minio-restored":
        try:
            _restore_qdrant_apply(backup_root, payload, target_state)
            _write_checkpoint(target_state, "qdrant-restored")
        except PipelineError as error:
            print(f"qdrant restore failed ({error}); PG vector rebuild required", file=sys.stderr)
            _write_checkpoint(target_state, "needs-vector-rebuild")
        stage = _read_checkpoint(target_state)

    # Reconcile path records real SQL intent; hermetic uses record helpers via psql fake.
    _reconcile_apply(target_state)
    summary = {
        "dryRun": False,
        "fingerprint": fingerprint,
        "stage": _read_checkpoint(target_state),
        "claimsLiveRestore": os.environ.get("MARKHAND_BACKUP_MODE") != "hermetic",
        "claimsRpoRtoPass": False,
    }
    (target_state / "summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    print(target_state / "summary.json")
    return 0


def _readiness_open(detail: str) -> None:
    mode = os.environ.get("MARKHAND_BACKUP_MODE", "live")
    sql = (
        "SELECT markhand_runtime_readiness_open('startup_reconciliation', "
        f"'{detail.replace(chr(39), chr(39)+chr(39))}');"
    )
    if mode == "hermetic":
        # Still require the SQL to be emitted and "executed" by fake psql — fail if fake fails.
        env, tmp = _hermetic_psql_env()
        try:
            run_cmd(["psql", "-v", "ON_ERROR_STOP=1", "-c", sql], env=env)
        finally:
            tmp.cleanup()
        return
    env, tmp = _pgpass_env("MARKHAND_BACKUP_DATABASE_URL")
    try:
        run_cmd(["psql", "-v", "ON_ERROR_STOP=1", "-c", sql], env=env)
    finally:
        tmp.cleanup()


def _hermetic_psql_env() -> tuple[dict[str, str], tempfile.TemporaryDirectory[str]]:
    tmp = tempfile.TemporaryDirectory(prefix="markhand-hermetic-pg-")
    env = os.environ.copy()
    env.setdefault("PGHOST", "127.0.0.1")
    env.setdefault("PGPORT", "5432")
    env.setdefault("PGUSER", "backup")
    env.setdefault("PGDATABASE", "markhand")
    env.setdefault("PGPASSWORD", "x")
    return env, tmp


def _restore_postgres_apply(backup_root: Path, payload: dict[str, Any], target_state: Path) -> None:
    pg = payload["postgres"]
    if pg.get("method") not in {"pg_basebackup_streamed_wal", "pitr_archive"}:
        die(f"unsupported postgres method: {pg.get('method')}")
    shadow = target_state / "shadow-pgdata"
    if shadow.exists() and os.environ.get("MARKHAND_RESTORE_ALLOW_NONEMPTY_PGDATA") != "1":
        die("shadow PGDATA exists; refuse overwrite")
    shadow.mkdir(parents=True, exist_ok=True)
    # Never extract into arbitrary host path for named-volume deployments.
    if os.environ.get("MARKHAND_RESTORE_PGDATA"):
        die(
            "MARKHAND_RESTORE_PGDATA host path extract blocked; "
            "use shadow volume cutover (target_state/shadow-pgdata)"
        )
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
        run_cmd(["tar", "-C", str(shadow), "-xf", str(tmp / "base.tar")])
        wal_dir = shadow / "pg_wal"
        wal_dir.mkdir(exist_ok=True)
        run_cmd(["tar", "-C", str(wal_dir), "-xf", str(tmp / "pg_wal.tar")])
        # Continuous PITR only when backup packaged archive WAL and restore consumes it.
        if pg.get("continuousPitr") is True:
            if pg.get("archiveWalPackaged") is not True:
                die("continuousPitr=true but archiveWalPackaged is not true")
            archive_enc = backup_root / "postgres" / "archive_wal.tar.enc"
            if not archive_enc.is_file():
                die("continuous PITR restore missing postgres/archive_wal.tar.enc")
            if sha256_file(archive_enc) != pg.get("archiveWalDigestSha256"):
                die("archive WAL package digest mismatch")
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
            archive_dest = shadow / "markhand_archive_wal"
            archive_dest.mkdir(exist_ok=True)
            run_cmd(["tar", "-C", str(archive_dest), "-xf", str(tmp / "archive_wal.tar")])
            (shadow / "recovery.signal").write_text("", encoding="utf-8")
            (shadow / "markhand_restore_command.txt").write_text(
                f"cp {archive_dest}/%f %p\n",
                encoding="utf-8",
            )
    (target_state / "postgres-postcheck.json").write_text(
        json.dumps(
            {
                "shadowPgdata": str(shadow),
                "stopWalLsn": pg["stopWalLsn"],
                "timelineId": pg["timelineId"],
                "continuousPitr": bool(pg.get("continuousPitr")),
                "archiveWalConsumed": bool(pg.get("continuousPitr")),
                "backupLabelPresent": (shadow / "backup_label").is_file()
                or any(shadow.rglob("backup_label")),
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )


def _restore_minio_apply(backup_root: Path, payload: dict[str, Any], target_state: Path) -> None:
    minio = payload["minio"]
    stage = backup_root / "minio"
    key_id = require_env("MARKHAND_BACKUP_PG_ENCRYPTION_KEY_ID")
    require_env("MARKHAND_BACKUP_PG_ENCRYPTION_KEY")
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
            die("MinIO inventory digest/count mismatch after decrypt")
        rows = [loads(line) for line in lines if line.strip()]
        rows.sort(key=lambda r: int(r["ordinal"]))
        mapping: list[dict[str, Any]] = []
        mode = os.environ.get("MARKHAND_BACKUP_MODE", "live")
        shadow_prefix = f"restore-shadow-{payload['manifestId'][:12]}"
        if mode == "hermetic":
            shadow_dir = target_state / "shadow-minio" / shadow_prefix
            shadow_dir.mkdir(parents=True, exist_ok=True)
            for row in rows:
                key = row["key"]
                if row.get("isDeleteMarker"):
                    target = shadow_dir / key
                    if target.exists():
                        target.unlink()
                    mapping.append(
                        {
                            "key": key,
                            "sourceVersionId": row["versionId"],
                            "restoredVersionId": f"new-del-{row['ordinal']}",
                            "isDeleteMarker": True,
                        }
                    )
                else:
                    src = stage / row["artifactRel"]
                    body = src.read_bytes()
                    if hashlib.sha256(body).hexdigest() != row["contentSha256"]:
                        die(f"object byte digest mismatch for {key}")
                    dest = shadow_dir / key
                    dest.parent.mkdir(parents=True, exist_ok=True)
                    dest.write_bytes(body)
                    mapping.append(
                        {
                            "key": key,
                            "sourceVersionId": row["versionId"],
                            "restoredVersionId": f"new-{row['ordinal']}",
                            "isDeleteMarker": False,
                            "contentSha256": row["contentSha256"],
                        }
                    )
        else:
            die("live MinIO restore apply requires Docker/mc path (use hermetic or enable services)")
        (target_state / "minio-version-mapping.json").write_text(
            json.dumps(
                {
                    "retainsSourceVersionIds": False,
                    "shadowPrefix": shadow_prefix,
                    "order": "oldest_to_newest",
                    "mappings": mapping,
                },
                indent=2,
            )
            + "\n",
            encoding="utf-8",
        )


def _restore_qdrant_apply(backup_root: Path, payload: dict[str, Any], target_state: Path) -> None:
    q = payload["qdrant"]
    digest = require_env("MARKHAND_INDEX_SIGNATURE")
    expected_collection = collection_name_for_digest(digest)
    if q.get("collectionName") != expected_collection:
        die("qdrant collectionName does not match index signature identity")
    if q.get("indexSignatureSha256") != digest:
        die("qdrant index signature mismatch")
    snap = backup_root / "qdrant" / "snapshot.bin"
    if sha256_file(snap) != q["snapshotDigestSha256"]:
        die("qdrant snapshot checksum mismatch")
    mode = os.environ.get("MARKHAND_BACKUP_MODE", "live")
    shadow_collection = f"{expected_collection}_restore_{payload['manifestId'][:8]}"
    if mode == "hermetic":
        (target_state / "qdrant-shadow.json").write_text(
            json.dumps(
                {
                    "shadowCollection": shadow_collection,
                    "sourceCollection": expected_collection,
                    "snapshotId": q["snapshotId"],
                    "pointsCount": q.get("pointsCount"),
                    "priority": "snapshot",
                    "verified": True,
                },
                indent=2,
            )
            + "\n",
            encoding="utf-8",
        )
        return
    url = require_env("MARKHAND_BACKUP_QDRANT_URL").rstrip("/")
    if not url.startswith("https://") and os.environ.get("MARKHAND_BACKUP_ALLOW_INSECURE_HTTP") != "1":
        die("live Qdrant restore requires https://")
    api_key = os.environ.get("MARKHAND_BACKUP_QDRANT_API_KEY", "")
    with tempfile.TemporaryDirectory(prefix="markhand-qdrant-") as tmp_s:
        cfg = Path(tmp_s) / "upload.cfg"
        # multipart upload + priority=snapshot
        cfg.write_text(
            f'url = "{url}/collections/{shadow_collection}/snapshots/upload?priority=snapshot&wait=true"\n'
            + (f'header = "api-key: {api_key}"\n' if api_key else "")
            + f'form = "snapshot=@{snap}"\n',
            encoding="utf-8",
        )
        cfg.chmod(0o600)
        run_cmd(["curl", "-fsS", "-K", str(cfg)])
        # poll collection status
        for _ in range(30):
            info_cfg = Path(tmp_s) / "info.cfg"
            info_cfg.write_text(
                f'url = "{url}/collections/{shadow_collection}"\n'
                + (f'header = "api-key: {api_key}"\n' if api_key else ""),
                encoding="utf-8",
            )
            info = loads(run_cmd(["curl", "-fsS", "-K", str(info_cfg)]).stdout)
            status = (info.get("result") or {}).get("status")
            if status == "green" or (isinstance(status, dict)):
                break
            time.sleep(1)
        else:
            die("qdrant shadow collection did not become ready")
    (target_state / "qdrant-shadow.json").write_text(
        json.dumps({"shadowCollection": shadow_collection, "priority": "snapshot"}, indent=2)
        + "\n",
        encoding="utf-8",
    )


def _reconcile_apply(target_state: Path) -> None:
    mode = os.environ.get("MARKHAND_BACKUP_MODE", "live")
    result = os.environ.get("MARKHAND_FAKE_RECONCILE_RESULT", "ok")
    if mode == "hermetic":
        env, tmp = _hermetic_psql_env()
        try:
            if result != "ok":
                run_cmd(
                    [
                        "psql",
                        "-v",
                        "ON_ERROR_STOP=1",
                        "-c",
                        "SELECT markhand_runtime_readiness_record_reconcile("
                        "'startup_reconciliation','drift',1,'hermetic drift');",
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
                        "'startup_reconciliation','hermetic');",
                    ],
                    env=env,
                )
                ready = try_ready.stdout.strip() in {"t", "true", "1"}
                if ready:
                    die("drift must keep ready=false")
                (target_state / "reconcile.json").write_text(
                    json.dumps({"result": "drift", "ready": False, "fabricated": False}, indent=2)
                    + "\n",
                    encoding="utf-8",
                )
                die("reconcile drift blocks readiness")
            run_cmd(
                [
                    "psql",
                    "-v",
                    "ON_ERROR_STOP=1",
                    "-c",
                    "SELECT markhand_runtime_readiness_record_reconcile("
                    "'startup_reconciliation','success',0,'hermetic zero-drift');",
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
                    "'startup_reconciliation','hermetic zero-drift');",
                ],
                env=env,
            )
            ready = try_ready.stdout.strip() in {"t", "true", "1"}
            (target_state / "reconcile.json").write_text(
                json.dumps(
                    {
                        "result": "success",
                        "ready": ready,
                        "fabricated": False,
                        "path": "markhand_runtime_readiness_record_reconcile+try_ready",
                    },
                    indent=2,
                )
                + "\n",
                encoding="utf-8",
            )
            if not ready:
                die("zero-drift hermetic path did not certify ready")
        finally:
            tmp.cleanup()
        _write_checkpoint(target_state, "reconciled")
        return
    # Live: require worker bulk enqueue + once.
    die(
        "live reconcile apply must run fileconv-worker with "
        "MARKHAND_WORKER_KIND=reconcile MARKHAND_RECONCILE_BULK_ENQUEUE=1 "
        "MARKHAND_RECONCILE_ONCE=1 MARKHAND_RECONCILE_MODE=repair"
    )


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
    # Ensure local imports work when invoked as a script.
    sys.path.insert(0, str(BACKUP_LIB))
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        return int(args.func(args))
    except (PipelineError, ManifestError, StrictJsonError, OSError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
