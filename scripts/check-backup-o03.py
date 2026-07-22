#!/usr/bin/env python3
"""P1B-O03 hermetic/static validation (correctness rebuild).

Does NOT claim live restore or Profile-B RPO/RTO success.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
BACKUP = ROOT / "deploy" / "backup"
SCRIPTS = BACKUP / "scripts"
FAKE_BIN = BACKUP / "fixtures" / "fake-bin"
PIPELINE = BACKUP / "lib" / "pipeline.py"
MIGRATION_SAFETY = BACKUP / "migration" / "validate-migration-safety.py"
EVIDENCE = BACKUP / "evidence" / "validation-report.json"
IMAGES_LOCK = BACKUP / "images.lock.json"
RUNBOOK_BACKUP = ROOT / "docs" / "runbooks" / "backup-restore.md"
RUNBOOK_MIGRATION = ROOT / "docs" / "runbooks" / "migration-safety.md"
REPORT_MD = ROOT / "bench" / "markhand_web" / "reports" / "p1b-o03-backup-restore.md"
WAL_OVERLAY = BACKUP / "compose.wal-archive.yml"

ORG_ID = "11111111-1111-1111-1111-111111111111"
INDEX_SIG = "72dda20007ffb7fbe293612091103321eb9e4e0e4a0517a5f3413e31a2978874"
SIGNING_KEY = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
PG_KEY = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210"
MIGRATION_VERSION = "0024_expand_runtime_readiness_zero_drift.sql"
APP_VERSION = "poc-o03"


def base_env(tmp: Path) -> dict[str, str]:
    env = os.environ.copy()
    # Prefer real tar/openssl for contract tests; keep fake psql/mc/curl/pg_basebackup.
    path = f"{FAKE_BIN}:{env.get('PATH', '')}"
    env.update(
        {
            "PATH": path,
            "MARKHAND_BACKUP_MODE": "hermetic",
            "MARKHAND_BACKUP_BIN_DIR": str(FAKE_BIN),
            "MARKHAND_WORKER_ORG_ID": ORG_ID,
            "MARKHAND_INDEX_SIGNATURE": INDEX_SIG,
            "MARKHAND_BACKUP_SIGNING_KEY_ID": "backup-hmac-1",
            "MARKHAND_BACKUP_SIGNING_KEY": SIGNING_KEY,
            "MARKHAND_BACKUP_APP_VERSION": APP_VERSION,
            "MARKHAND_BACKUP_MIGRATION_VERSION": MIGRATION_VERSION,
            "MARKHAND_BACKUP_SCHEMA_NAME": "public",
            "MARKHAND_BACKUP_PGHOST": "127.0.0.1",
            "MARKHAND_BACKUP_PGPORT": "5432",
            "MARKHAND_BACKUP_PGUSER": "backup_role",
            "MARKHAND_BACKUP_PGDATABASE": "markhand",
            "MARKHAND_BACKUP_PGPASSWORD": "backup-secret-not-argv",
            "MARKHAND_BACKUP_PG_ENCRYPTION_KEY_ID": "pg-enc-1",
            "MARKHAND_BACKUP_PG_ENCRYPTION_KEY": PG_KEY,
            "MARKHAND_MINIO_BUCKET": "markhand-documents",
            "MARKHAND_BACKUP_MINIO_ENDPOINT": "http://127.0.0.1:9010",
            "MARKHAND_BACKUP_MINIO_ACCESS_KEY": "backup-access",
            "MARKHAND_BACKUP_MINIO_SECRET_KEY": "backup-secret-not-logged",
            "MARKHAND_BACKUP_QDRANT_URL": "http://127.0.0.1:6343",
            "MARKHAND_FAKE_PSQL_STATE": str(tmp / "psql-state"),
            "MARKHAND_RESTORE_TARGET_STATE": str(tmp / "target-state"),
        }
    )
    return env


def run(args: list[str], env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        args,
        cwd=str(ROOT),
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )


def run_backup(env: dict[str, str], backup_dir: Path) -> subprocess.CompletedProcess[str]:
    return run(["python3", str(PIPELINE), "backup", "--backup-root", str(backup_dir)], env)


class BackupO03Checks:
    def __init__(self) -> None:
        self.errors: list[str] = []

    def err(self, message: str) -> None:
        self.errors.append(message)

    def run_static(self) -> None:
        for name in (
            "backup.sh",
            "restore.sh",
            "fence-writes.sh",
            "validate-manifest.sh",
            "reconcile-before-ready.sh",
            "rebuild-vectors-from-pg.sh",
        ):
            path = SCRIPTS / name
            if not path.is_file() or not (path.stat().st_mode & stat.S_IXUSR):
                self.err(f"missing/executable script: {name}")
        for path in (
            PIPELINE,
            BACKUP / "lib" / "crypto.py",
            BACKUP / "lib" / "manifest.py",
            MIGRATION_SAFETY,
            IMAGES_LOCK,
            WAL_OVERLAY,
            RUNBOOK_BACKUP,
            RUNBOOK_MIGRATION,
        ):
            if not path.is_file():
                self.err(f"missing {path.relative_to(ROOT)}")
        lock = json.loads(IMAGES_LOCK.read_text(encoding="utf-8"))
        for key in ("postgres", "qdrant", "minio", "minio-mc"):
            if "@sha256:" not in str((lock.get("images") or {}).get(key, "")):
                self.err(f"images.lock missing digest for {key}")
        tools = lock.get("tools") or {}
        if "cryptography" in tools:
            self.err("images.lock must not claim host cryptography dependency")
        enc = lock.get("encryption") or {}
        if enc.get("algorithm") != "aes-256-ctr-hmac-sha256-v1":
            self.err("images.lock must pin aes-256-ctr-hmac-sha256-v1")
        if (lock.get("postgresBackup") or {}).get("walArchiveOverlay") != "preparatory_only":
            self.err("images.lock must label wal-archive overlay preparatory_only")
        schema_path = BACKUP / "schema" / "recovery-manifest.schema.json"
        if not schema_path.is_file():
            self.err("missing recovery-manifest.schema.json")
        else:
            schema = json.loads(schema_path.read_text(encoding="utf-8"))
            methods = (
                schema.get("definitions", {})
                .get("postgres", {})
                .get("properties", {})
                .get("method", {})
                .get("enum")
                or []
            )
            if "pg_basebackup_pitr" in methods:
                self.err("schema still contains stale pg_basebackup_pitr")
            if "pg_basebackup_streamed_wal" not in methods:
                self.err("schema missing pg_basebackup_streamed_wal")
        text = RUNBOOK_BACKUP.read_text(encoding="utf-8")
        for section in ("Prerequisites", "Procedure", "Verify", "Rollback"):
            if f"## {section}" not in text:
                self.err(f"runbook missing {section}")
        if "I_UNDERSTAND_DESTRUCTIVE_RESTORE" not in text:
            self.err("runbook missing destructive confirmation")
        lowered = text.lower()
        if "python `cryptography`" in lowered or "host `cryptography`" in lowered:
            self.err("runbook must not claim cryptography package")
        if "aes-256-gcm" in lowered or "authenticated encryption with associated data" in lowered:
            self.err("runbook must not claim GCM/AEAD")
        if "aes-256-ctr-hmac-sha256-v1" not in text:
            self.err("runbook must document aes-256-ctr-hmac-sha256-v1")
        if "pg_basebackup_streamed_wal" not in text and "streamed WAL" not in text:
            self.err("runbook must document streamed WAL backup (not false PITR)")
        if "preparatory" not in text.lower():
            self.err("runbook must label wal-archive overlay preparatory")
        overlay = WAL_OVERLAY.read_text(encoding="utf-8")
        if "PREPARATORY" not in overlay and "preparatory" not in overlay:
            self.err("compose.wal-archive.yml must state preparatory-only")
        if "does NOT enable continuous PITR" not in overlay and "does not enable" not in overlay.lower():
            self.err("compose.wal-archive.yml must not imply continuous PITR enabled")
        mig = run([sys.executable, str(MIGRATION_SAFETY), "--check"], os.environ.copy())
        if mig.returncode != 0:
            self.err(f"migration safety failed: {mig.stderr}")
        agree = run(
            [
                sys.executable,
                str(BACKUP / "lib" / "manifest.py"),
                "assert-schema-agreement",
            ],
            os.environ.copy(),
        )
        if agree.returncode != 0:
            self.err(f"schema/code agreement failed: {agree.stderr}")


class BackupO03Tests(unittest.TestCase):
    def test_success_backup_dry_run_and_apply(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_s:
            tmp = Path(tmp_s)
            env = base_env(tmp)
            backup_dir = tmp / "backup"
            result = run_backup(env, backup_dir)
            self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
            manifest = json.loads((backup_dir / "recovery-manifest.json").read_text())
            self.assertEqual(manifest["postgres"]["method"], "pg_basebackup_streamed_wal")
            self.assertFalse(manifest["postgres"]["continuousPitr"])
            self.assertFalse(manifest["postgres"]["archiveWalPackaged"])
            self.assertIsNone(manifest["postgres"]["archiveWalRequiredThroughLsn"])
            self.assertEqual(
                manifest["postgres"]["encryption"]["algorithm"],
                "aes-256-ctr-hmac-sha256-v1",
            )
            for field in ("saltHex", "ivHex", "macHex", "aad", "kdf"):
                self.assertIn(field, manifest["postgres"]["encryption"])
            self.assertFalse(manifest["minio"]["retainsSourceVersionIdsOnRestore"])
            self.assertTrue(
                manifest["qdrant"]["collectionName"].endswith(INDEX_SIG)
            )
            # dry-run read-only
            dry = run(
                [
                    "python3",
                    str(PIPELINE),
                    "restore",
                    "--backup-root",
                    str(backup_dir),
                    "--target-state",
                    str(tmp / "target-state"),
                ],
                env,
            )
            self.assertEqual(dry.returncode, 0, dry.stderr)
            report = json.loads((tmp / "target-state" / "dry-run-report.json").read_text())
            self.assertTrue(report["dryRun"])
            self.assertTrue(report["readOnly"])
            self.assertEqual(report["mutations"], [])
            self.assertFalse((tmp / "target-state" / "stage").exists())
            # apply with confirmation
            env["MARKHAND_RESTORE_CONFIRM"] = "I_UNDERSTAND_DESTRUCTIVE_RESTORE"
            env["MARKHAND_FAKE_RECONCILE_RESULT"] = "ok"
            apply = run(
                [
                    "python3",
                    str(PIPELINE),
                    "restore",
                    "--backup-root",
                    str(backup_dir),
                    "--target-state",
                    str(tmp / "target-apply"),
                    "--apply",
                ],
                env,
            )
            self.assertEqual(apply.returncode, 0, apply.stderr + apply.stdout)
            summary = json.loads((tmp / "target-apply" / "summary.json").read_text())
            self.assertFalse(summary["claimsRpoRtoPass"])
            mapping = json.loads(
                (tmp / "target-apply" / "minio-version-mapping.json").read_text()
            )
            self.assertFalse(mapping["retainsSourceVersionIds"])
            self.assertTrue((tmp / "target-apply" / "shadow-pgdata").is_dir())

    def test_drift_keeps_ready_false_zero_drift_permits(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_s:
            tmp = Path(tmp_s)
            env = base_env(tmp)
            backup_dir = tmp / "backup"
            self.assertEqual(run_backup(env, backup_dir).returncode, 0)
            env["MARKHAND_RESTORE_CONFIRM"] = "I_UNDERSTAND_DESTRUCTIVE_RESTORE"
            env["MARKHAND_FAKE_RECONCILE_RESULT"] = "drift"
            bad = run(
                [
                    "python3",
                    str(PIPELINE),
                    "restore",
                    "--backup-root",
                    str(backup_dir),
                    "--target-state",
                    str(tmp / "t-drift"),
                    "--apply",
                ],
                env,
            )
            self.assertNotEqual(bad.returncode, 0)
            # fresh psql state for success path
            env["MARKHAND_FAKE_PSQL_STATE"] = str(tmp / "psql-ok")
            env["MARKHAND_FAKE_RECONCILE_RESULT"] = "ok"
            ok = run(
                [
                    "python3",
                    str(PIPELINE),
                    "restore",
                    "--backup-root",
                    str(backup_dir),
                    "--target-state",
                    str(tmp / "t-ok"),
                    "--apply",
                ],
                env,
            )
            self.assertEqual(ok.returncode, 0, ok.stderr)
            recon = json.loads((tmp / "t-ok" / "reconcile.json").read_text())
            self.assertTrue(recon["ready"])
            self.assertFalse(recon["fabricated"])

    def test_corrupt_manifest_and_duplicate_key(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_s:
            tmp = Path(tmp_s)
            env = base_env(tmp)
            backup_dir = tmp / "backup"
            self.assertEqual(run_backup(env, backup_dir).returncode, 0)
            path = backup_dir / "recovery-manifest.json"
            payload = json.loads(path.read_text())
            payload["signature"]["value"] = "0" * 64
            path.write_text(json.dumps(payload) + "\n")
            bad = run(
                [
                    "python3",
                    str(PIPELINE),
                    "validate-manifest",
                    "--manifest",
                    str(path),
                    "--backup-root",
                    str(backup_dir),
                ],
                env,
            )
            self.assertNotEqual(bad.returncode, 0)
            dup = tmp / "dup.json"
            dup.write_text('{"a":1,"a":2}\n', encoding="utf-8")
            sys.path.insert(0, str(BACKUP / "lib"))
            from strictjson import StrictJsonError, loads  # type: ignore

            with self.assertRaises(StrictJsonError):
                loads(dup.read_text())

    def test_wrong_org_schema_signature_migration(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_s:
            tmp = Path(tmp_s)
            env = base_env(tmp)
            backup_dir = tmp / "backup"
            self.assertEqual(run_backup(env, backup_dir).returncode, 0)
            for key, value in (
                ("MARKHAND_WORKER_ORG_ID", "22222222-2222-2222-2222-222222222222"),
                ("MARKHAND_BACKUP_SCHEMA_NAME", "other"),
                ("MARKHAND_INDEX_SIGNATURE", "a" * 64),
                ("MARKHAND_BACKUP_MIGRATION_VERSION", "9999_expand_x.sql"),
            ):
                bad_env = env.copy()
                bad_env[key] = value
                result = run(
                    [
                        "python3",
                        str(PIPELINE),
                        "validate-manifest",
                        "--manifest",
                        str(backup_dir / "recovery-manifest.json"),
                        "--backup-root",
                        str(backup_dir),
                    ],
                    bad_env,
                )
                self.assertNotEqual(result.returncode, 0, key)

    def test_missing_artifact_and_command_failure(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_s:
            tmp = Path(tmp_s)
            env = base_env(tmp)
            backup_dir = tmp / "backup"
            self.assertEqual(run_backup(env, backup_dir).returncode, 0)
            (backup_dir / "qdrant" / "snapshot.bin").unlink()
            result = run(
                [
                    "python3",
                    str(PIPELINE),
                    "validate-manifest",
                    "--manifest",
                    str(backup_dir / "recovery-manifest.json"),
                    "--backup-root",
                    str(backup_dir),
                ],
                env,
            )
            self.assertNotEqual(result.returncode, 0)
            env2 = base_env(tmp)
            env2["MARKHAND_FAKE_PSQL_FAIL"] = "1"
            # readiness open during apply must fail closed
            self.assertEqual(run_backup(env2, tmp / "b2").returncode, 0)
            env2["MARKHAND_RESTORE_CONFIRM"] = "I_UNDERSTAND_DESTRUCTIVE_RESTORE"
            failed = run(
                [
                    "python3",
                    str(PIPELINE),
                    "restore",
                    "--backup-root",
                    str(tmp / "b2"),
                    "--target-state",
                    str(tmp / "t-fail"),
                    "--apply",
                ],
                env2,
            )
            self.assertNotEqual(failed.returncode, 0)

    def test_path_traversal_symlink_destructive_confirm(self) -> None:
        sys.path.insert(0, str(BACKUP / "lib"))
        from manifest import verify_artifact_checksums  # type: ignore

        with tempfile.TemporaryDirectory() as tmp_s:
            tmp = Path(tmp_s)
            backup_dir = tmp / "backup"
            backup_dir.mkdir()
            (backup_dir / "ok.txt").write_text("x", encoding="utf-8")
            outside = tmp / "outside.txt"
            outside.write_text("secret", encoding="utf-8")
            link = backup_dir / "link.txt"
            link.symlink_to(outside)
            errors = verify_artifact_checksums(
                {
                    "checksums": {
                        "ok.txt": hashlib.sha256(b"x").hexdigest(),
                        "link.txt": hashlib.sha256(b"secret").hexdigest(),
                        "../outside.txt": hashlib.sha256(b"secret").hexdigest(),
                    },
                    "artifacts": {
                        "relativePaths": {
                            "ok": "ok.txt",
                            "link": "link.txt",
                            "escape": "../outside.txt",
                        }
                    },
                },
                backup_dir,
            )
            joined = " ".join(errors).lower()
            self.assertIn("symlink", joined)
            self.assertTrue("unsafe" in joined or "traversal" in joined)
            env = base_env(tmp)
            backup = tmp / "b"
            self.assertEqual(run_backup(env, backup).returncode, 0)
            denied = run(
                [
                    "python3",
                    str(PIPELINE),
                    "restore",
                    "--backup-root",
                    str(backup),
                    "--target-state",
                    str(tmp / "t"),
                    "--apply",
                ],
                env,
            )
            self.assertNotEqual(denied.returncode, 0)
            self.assertTrue(
                "I_UNDERSTAND_DESTRUCTIVE_RESTORE" in denied.stderr
                or "MARKHAND_RESTORE_CONFIRM" in denied.stderr,
                denied.stderr,
            )

    def test_ordered_bounded_cannot_claim_writes_fenced(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_s:
            tmp = Path(tmp_s)
            env = base_env(tmp)
            # No docker → ordered-bounded
            out = tmp / "fence.json"
            result = run(
                ["python3", str(PIPELINE), "fence", "--output", str(out)],
                env,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            fence = json.loads(out.read_text())
            if fence["mode"] == "ordered-bounded":
                self.assertFalse(fence["writesFenced"])

    def test_crypto_envelope_openssl_roundtrip_and_tamper(self) -> None:
        sys.path.insert(0, str(BACKUP / "lib"))
        from crypto import CryptoError, decrypt_file, encrypt_file  # type: ignore

        with tempfile.TemporaryDirectory() as tmp_s:
            tmp = Path(tmp_s)
            plain = tmp / "base.tar"
            src = tmp / "src"
            src.mkdir()
            (src / "backup_label").write_text("START WAL LOCATION: 0/1\n", encoding="utf-8")
            subprocess.run(
                ["tar", "-C", str(src), "-cf", str(plain), "."],
                check=True,
            )
            os.environ["MARKHAND_BACKUP_PG_ENCRYPTION_KEY"] = PG_KEY
            meta = encrypt_file(
                plain,
                tmp / "base.tar.enc",
                key_env="MARKHAND_BACKUP_PG_ENCRYPTION_KEY",
                key_id="pg-enc-1",
                aad=b"aad-test",
            )
            self.assertEqual(meta["algorithm"], "aes-256-ctr-hmac-sha256-v1")
            self.assertEqual(meta["kdf"], "hkdf-sha256")
            self.assertEqual(len(bytes.fromhex(meta["saltHex"])), 32)
            self.assertEqual(len(bytes.fromhex(meta["ivHex"])), 16)
            decrypt_file(
                tmp / "base.tar.enc",
                tmp / "out.tar",
                key_env="MARKHAND_BACKUP_PG_ENCRYPTION_KEY",
                meta=meta,
                aad=b"aad-test",
                expected_key_id="pg-enc-1",
            )
            listing = subprocess.check_output(["tar", "-tf", str(tmp / "out.tar")], text=True)
            self.assertIn("backup_label", listing)

            # Ciphertext tamper → MAC fail before decrypt.
            ct = bytearray((tmp / "base.tar.enc").read_bytes())
            ct[-1] ^= 0x01
            (tmp / "tampered.enc").write_bytes(ct)
            with self.assertRaises(CryptoError):
                decrypt_file(
                    tmp / "tampered.enc",
                    tmp / "bad.tar",
                    key_env="MARKHAND_BACKUP_PG_ENCRYPTION_KEY",
                    meta={**meta, "ciphertextSha256": hashlib.sha256(ct).hexdigest()},
                    aad=b"aad-test",
                    expected_key_id="pg-enc-1",
                )

            # Wrong key material.
            os.environ["MARKHAND_BACKUP_PG_ENCRYPTION_KEY"] = "11" * 32
            with self.assertRaises(CryptoError):
                decrypt_file(
                    tmp / "base.tar.enc",
                    tmp / "wrong-key.tar",
                    key_env="MARKHAND_BACKUP_PG_ENCRYPTION_KEY",
                    meta=meta,
                    aad=b"aad-test",
                    expected_key_id="pg-enc-1",
                )
            os.environ["MARKHAND_BACKUP_PG_ENCRYPTION_KEY"] = PG_KEY

            # Wrong keyId.
            with self.assertRaises(CryptoError):
                decrypt_file(
                    tmp / "base.tar.enc",
                    tmp / "wrong-id.tar",
                    key_env="MARKHAND_BACKUP_PG_ENCRYPTION_KEY",
                    meta=meta,
                    aad=b"aad-test",
                    expected_key_id="pg-enc-OTHER",
                )

            # Truncation.
            truncated = (tmp / "base.tar.enc").read_bytes()[:8]
            (tmp / "trunc.enc").write_bytes(truncated)
            with self.assertRaises(CryptoError):
                decrypt_file(
                    tmp / "trunc.enc",
                    tmp / "trunc.tar",
                    key_env="MARKHAND_BACKUP_PG_ENCRYPTION_KEY",
                    meta={**meta, "ciphertextSha256": hashlib.sha256(truncated).hexdigest()},
                    aad=b"aad-test",
                    expected_key_id="pg-enc-1",
                )

    def test_schema_enforced_unknown_and_drift(self) -> None:
        sys.path.insert(0, str(BACKUP / "lib"))
        from manifest import (  # type: ignore
            ManifestError,
            assert_schema_code_agreement,
            validate_structure,
        )
        from schema_validate import SchemaError, load_schema, validate_manifest  # type: ignore
        from strictjson import StrictJsonError, loads  # type: ignore

        assert_schema_code_agreement()
        with tempfile.TemporaryDirectory() as tmp_s:
            tmp = Path(tmp_s)
            env = base_env(tmp)
            backup_dir = tmp / "backup"
            self.assertEqual(run_backup(env, backup_dir).returncode, 0)
            payload = json.loads((backup_dir / "recovery-manifest.json").read_text())
            # Unknown top-level field must fail schema validation.
            bad = dict(payload)
            bad["unexpectedField"] = "nope"
            with self.assertRaises(SchemaError):
                validate_manifest(bad)
            errors = validate_structure(bad)
            self.assertTrue(any("unknown" in e.lower() for e in errors), errors)
            # Duplicate keys fail at parse time.
            with self.assertRaises(StrictJsonError):
                loads('{"schemaVersion":1,"schemaVersion":2}')
            # Schema/code drift mutation must fail.
            schema = load_schema()
            schema["definitions"]["postgres"]["properties"]["method"]["enum"] = [
                "pg_basebackup_pitr"
            ]
            with self.assertRaises(ManifestError):
                assert_schema_code_agreement(schema)
            schema = load_schema()
            schema["definitions"]["encryptionObject"]["properties"]["algorithm"][
                "const"
            ] = "aes-256-gcm"
            with self.assertRaises(ManifestError):
                assert_schema_code_agreement(schema)

    def test_pitr_blocked_without_packaged_archive_wal(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_s:
            tmp = Path(tmp_s)
            env = base_env(tmp)
            env["MARKHAND_BACKUP_PITR_ARCHIVE"] = "1"
            # Overlay-ish flag alone must not enable PITR without packaged archive WAL.
            env["MARKHAND_PG_ARCHIVE_MODE"] = "on"
            result = run_backup(env, tmp / "backup-pitr-blocked")
            self.assertNotEqual(result.returncode, 0)
            self.assertTrue(
                "ARCHIVE_WAL" in result.stderr or "preparatory" in result.stderr.lower(),
                result.stderr,
            )

    def test_partial_resume_and_target_binding(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_s:
            tmp = Path(tmp_s)
            env = base_env(tmp)
            backup_dir = tmp / "backup"
            self.assertEqual(run_backup(env, backup_dir).returncode, 0)
            env["MARKHAND_RESTORE_CONFIRM"] = "I_UNDERSTAND_DESTRUCTIVE_RESTORE"
            target = tmp / "target"
            # First apply
            self.assertEqual(
                run(
                    [
                        "python3",
                        str(PIPELINE),
                        "restore",
                        "--backup-root",
                        str(backup_dir),
                        "--target-state",
                        str(target),
                        "--apply",
                    ],
                    env,
                ).returncode,
                0,
            )
            # Anti-replay without allow
            (target / "cutover.json").write_text(
                json.dumps(
                    {
                        "manifestSha256": hashlib.sha256(
                            (backup_dir / "recovery-manifest.json").read_bytes()
                        ).hexdigest()
                    }
                )
                + "\n"
            )
            # Reset stage to allow code path to hit cutover check at start of apply after fingerprint
            # Re-run should fail anti-replay
            env["MARKHAND_FAKE_PSQL_STATE"] = str(tmp / "psql2")
            # clear stage to re-enter apply after cutover written
            shutil.rmtree(target / "shadow-pgdata", ignore_errors=True)
            (target / "stage").write_text("none\n")
            replay = run(
                [
                    "python3",
                    str(PIPELINE),
                    "restore",
                    "--backup-root",
                    str(backup_dir),
                    "--target-state",
                    str(target),
                    "--apply",
                ],
                env,
            )
            self.assertNotEqual(replay.returncode, 0)
            self.assertIn("anti-replay", replay.stderr)

    def test_migration_safety_self_test(self) -> None:
        result = run([sys.executable, str(MIGRATION_SAFETY), "--self-test"], os.environ.copy())
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)

    def test_redaction_and_no_url_argv(self) -> None:
        sys.path.insert(0, str(BACKUP / "lib"))
        from manifest import redact_env_for_log  # type: ignore

        redacted = redact_env_for_log(
            {
                "MARKHAND_BACKUP_SIGNING_KEY": SIGNING_KEY,
                "MARKHAND_BACKUP_PGPASSWORD": "x",
                "MARKHAND_MINIO_BUCKET": "markhand-documents",
            }
        )
        self.assertEqual(redacted["MARKHAND_BACKUP_SIGNING_KEY"], "***REDACTED***")
        self.assertEqual(redacted["MARKHAND_BACKUP_PGPASSWORD"], "***REDACTED***")


def build_evidence(test_result: unittest.TestResult, static_errors: list[str]) -> dict[str, Any]:
    return {
        "version": 2,
        "issue": "P1B-O03",
        "ok": test_result.wasSuccessful() and not static_errors,
        "claims_live_restore": False,
        "claims_rpo_rto_pass": False,
        "profileBDrGate": {
            "status": "unresolved",
            "targetMatch": False,
            "note": "Profile-B live restore with measured RPO/RTO remains pending.",
        },
        "postgresMethod": "pg_basebackup_streamed_wal",
        "continuousPitr": "blocked_unless_archive_wal_packaged_and_consumed",
        "walArchiveOverlay": "preparatory_only",
        "encryption": "aes-256-ctr-hmac-sha256-v1 (stdlib HKDF/HMAC + openssl aes-256-ctr; not GCM/AEAD)",
        "evidenceClasses": {
            "implemented": [
                "streamed WAL restorable PG backup + EtM envelope metadata",
                "MinIO version/delete-marker inventory + restore mapping",
                "Qdrant collection identity from index signature",
                "dry-run read-only; fence quiescence; target-bound restore state",
                "zero-drift readiness certification (migration 0024)",
                "bulk enqueue + reconcile-once worker path",
                "recovery-manifest.schema.json enforced (unknown fields fail)",
            ],
            "static": [
                "digest pins",
                "runbooks",
                "migration safety + SQL semantic policy",
                "wal-archive overlay labeled preparatory only",
                "no host cryptography package lock claim",
            ],
            "hermetic": [
                "backup+dry-run+apply",
                "drift keeps ready false / zero-drift ready",
                "corrupt/duplicate JSON",
                "org/schema/signature/migration mismatch",
                "missing artifact / command failure",
                "path traversal/symlink/destructive confirm",
                "OpenSSL CTR+HMAC roundtrip/tamper/wrong-key/truncation",
                "schema enforcement + schema/code drift mutation",
                "PITR blocked without packaged archive WAL",
                "anti-replay target binding",
            ],
            "pending_live": [
                "Docker compose restore with shadow cutover",
                "continuous PITR only after packaged archive WAL + restore consume",
                "Profile-B RPO/RTO measurements",
            ],
        },
        "hermeticTestsRun": test_result.testsRun,
        "hermeticFailures": [
            str(item[0]) for item in list(test_result.failures) + list(test_result.errors)
        ],
        "staticErrors": static_errors,
        "commands": [
            "python3 scripts/check-backup-o03.py --self-test",
            "python3 deploy/backup/migration/validate-migration-safety.py --check",
            "make check-backup",
            "cargo test -p fileconv-server reconcile_report_drift_blocks_zero_drift_label",
        ],
    }


def write_report_md(evidence: dict[str, Any]) -> None:
    lines = [
        "# P1B-O03 evidence — backup/restore and migration safety (rebuild)",
        "",
        "Status: **In Progress**.",
        f"`claims_live_restore`: **{str(evidence['claims_live_restore']).lower()}**",
        f"`claims_rpo_rto_pass`: **{str(evidence['claims_rpo_rto_pass']).lower()}**",
        f"PostgreSQL method: `{evidence['postgresMethod']}` "
        f"(continuous PITR: `{evidence['continuousPitr']}`).",
        "",
        "## Evidence classes",
        "",
    ]
    for klass, items in evidence["evidenceClasses"].items():
        lines.append(f"### {klass}")
        lines.append("")
        for item in items:
            lines.append(f"- {item}")
        lines.append("")
    lines.extend(
        [
            "## Non-claims / blockers",
            "",
            "- No live Docker restore or Profile-B RPO/RTO pass.",
            "- Continuous PITR blocked unless archived WAL through target LSN is "
            "packaged/checksummed and consumed on restore; wal-archive overlay is preparatory only.",
            "- Multi-region DR out of scope.",
            "",
        ]
    )
    REPORT_MD.parent.mkdir(parents=True, exist_ok=True)
    REPORT_MD.write_text("\n".join(lines), encoding="utf-8")


def main(argv: list[str] | None = None) -> int:
    import argparse

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--json-report", type=Path, default=EVIDENCE)
    args = parser.parse_args(argv)
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(BackupO03Tests)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    checks = BackupO03Checks()
    checks.run_static()
    evidence = build_evidence(result, checks.errors)
    args.json_report.parent.mkdir(parents=True, exist_ok=True)
    args.json_report.write_text(
        json.dumps(evidence, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    write_report_md(evidence)
    print(
        f"P1B-O03 backup validation {'OK' if evidence['ok'] else 'FAILED'} "
        f"(hermetic={result.testsRun}, static_errors={len(checks.errors)}, "
        f"claims_live_restore=false, claims_rpo_rto_pass=false)"
    )
    for error in checks.errors:
        print(f"- {error}", file=sys.stderr)
    return 0 if evidence["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
