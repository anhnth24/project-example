#!/usr/bin/env python3
"""P1B-O03 final-round validation — production adapters + stateful fakes.

Does NOT claim live restore or Profile-B RPO/RTO success. Status: In Progress.
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
    path = f"{FAKE_BIN}:{env.get('PATH', '')}"
    env.update(
        {
            "PATH": path,
            "MARKHAND_BACKUP_BIN_DIR": str(FAKE_BIN),
            "MARKHAND_WORKER_ORG_ID": ORG_ID,
            "MARKHAND_INDEX_SIGNATURE": INDEX_SIG,
            "MARKHAND_BACKUP_SIGNING_KEY_ID": "backup-hmac-1",
            "MARKHAND_BACKUP_SIGNING_KEY": SIGNING_KEY,
            "MARKHAND_BACKUP_APP_VERSION": APP_VERSION,
            "MARKHAND_BACKUP_COMPAT_APP_MIN": APP_VERSION,
            "MARKHAND_BACKUP_COMPAT_APP_MAX": APP_VERSION,
            "MARKHAND_BACKUP_MIGRATION_VERSION": MIGRATION_VERSION,
            "MARKHAND_BACKUP_SCHEMA_NAME": "public",
            "MARKHAND_BACKUP_PGHOST": "127.0.0.1",
            "MARKHAND_BACKUP_PGPORT": "5432",
            "MARKHAND_BACKUP_PGUSER": "backup_role",
            "MARKHAND_BACKUP_PGDATABASE": "markhand",
            "MARKHAND_BACKUP_PGPASSWORD": "backup-secret-not-argv",
            "MARKHAND_BACKUP_ALLOW_INSECURE_PG": "1",
            "MARKHAND_BACKUP_PG_ENCRYPTION_KEY_ID": "pg-enc-1",
            "MARKHAND_BACKUP_PG_ENCRYPTION_KEY": PG_KEY,
            "MARKHAND_MINIO_BUCKET": "markhand-documents",
            "MARKHAND_BACKUP_MINIO_ENDPOINT": "http://127.0.0.1:9010",
            "MARKHAND_BACKUP_MINIO_ACCESS_KEY": "backup-access",
            "MARKHAND_BACKUP_MINIO_SECRET_KEY": "backup-secret-not-logged",
            "MARKHAND_BACKUP_ALLOW_INSECURE_HTTP": "1",
            "MARKHAND_BACKUP_QDRANT_URL": "http://127.0.0.1:6333",
            "MARKHAND_BACKUP_ALLOW_ORDERED_BOUNDED": "1",
            "MARKHAND_BACKUP_ORDERED_BOUNDED_MAX_SECS": "120",
            "MARKHAND_BACKUP_PG_CTL": str(FAKE_BIN / "pg_ctl_shadow"),
            "MARKHAND_FAKE_PSQL_STATE": str(tmp / "psql-state"),
            "MARKHAND_FAKE_CURL_STATE": str(tmp / "curl-state"),
            "MARKHAND_FAKE_MC_STATE": str(tmp / "mc-state"),
            "MARKHAND_FAKE_PG_CTL_STATE": str(tmp / "pgctl-state"),
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
            path = BACKUP / "scripts" / name
            if not path.is_file() or not (path.stat().st_mode & stat.S_IXUSR):
                self.err(f"missing/executable script: {name}")
        for path in (
            PIPELINE,
            BACKUP / "lib" / "crypto.py",
            BACKUP / "lib" / "pg_wal.py",
            BACKUP / "lib" / "campaign.py",
            BACKUP / "lib" / "qdrant_api.py",
            BACKUP / "lib" / "sql_lexer.py",
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
        if "cryptography" in (lock.get("tools") or {}):
            self.err("images.lock must not claim cryptography")
        if (lock.get("postgresBackup") or {}).get("walArchiveOverlay") != "preparatory_only":
            self.err("wal overlay must be preparatory_only")
        text = RUNBOOK_BACKUP.read_text(encoding="utf-8")
        for section in ("Prerequisites", "Procedure", "Verify", "Rollback"):
            if f"## {section}" not in text:
                self.err(f"runbook missing {section}")
        if "I_UNDERSTAND_DESTRUCTIVE_RESTORE" not in text:
            self.err("runbook missing destructive confirmation")
        if "preparatory" not in text.lower():
            self.err("runbook must label wal-archive overlay preparatory")
        if "aes-256-ctr-hmac-sha256-v1" not in text:
            self.err("runbook must document EtM algorithm id")
        overlay = WAL_OVERLAY.read_text(encoding="utf-8")
        if "preparatory" not in overlay.lower():
            self.err("compose.wal-archive.yml must be preparatory")
        mig = run(
            [
                sys.executable,
                str(MIGRATION_SAFETY),
                "--check",
                "--base-ref",
                os.environ.get(
                    "MARKHAND_MIGRATION_BASE_REF",
                    subprocess.check_output(
                        ["git", "merge-base", "HEAD", "origin/master"],
                        cwd=str(ROOT),
                        text=True,
                    ).strip(),
                ),
            ],
            os.environ.copy(),
        )
        if mig.returncode != 0:
            self.err(f"migration safety failed: {mig.stderr}")


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
            self.assertIn("walCoverage", manifest["postgres"])
            self.assertEqual(
                manifest["postgres"]["encryption"]["algorithm"],
                "aes-256-ctr-hmac-sha256-v1",
            )
            self.assertTrue(manifest["minio"]["objectBodiesEncrypted"])
            self.assertIn("compatibleAppVersionRange", manifest)
            self.assertEqual(manifest["qdrant"]["status"], "green")
            self.assertIsInstance(manifest["qdrant"]["indexedVectorsCount"], int)
            # dry-run
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
            self.assertEqual(report["mutations"], [])
            # apply
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
            self.assertTrue((tmp / "target-apply" / "pg-recovery-state.json").is_file())
            recovery = json.loads((tmp / "target-apply" / "pg-recovery-state.json").read_text())
            self.assertTrue(recovery["recovered"])
            self.assertTrue((tmp / "target-apply" / "cutover-receipt.json").is_file())
            receipt = json.loads((tmp / "target-apply" / "cutover-receipt.json").read_text())
            self.assertTrue(receipt["operations"])
            mapping = json.loads(
                (tmp / "target-apply" / "minio-version-mapping.json").read_text()
            )
            self.assertFalse(mapping["retainsSourceVersionIds"])
            self.assertEqual(mapping["order"], "oldest_to_newest")

    def test_junk_wal_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_s:
            tmp = Path(tmp_s)
            env = base_env(tmp)
            env["MARKHAND_FAKE_PG_JUNK_WAL"] = "1"
            result = run_backup(env, tmp / "backup")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("junk", result.stderr.lower())

    def test_qdrant_real_schema_parser(self) -> None:
        sys.path.insert(0, str(BACKUP / "lib"))
        from qdrant_api import QdrantApiError, parse_collection_info  # type: ignore

        ok = parse_collection_info(
            {
                "result": {
                    "status": "green",
                    "points_count": 3,
                    "indexed_vectors_count": 3,
                    "config": {"params": {"vectors": {"size": 8, "distance": "Cosine"}}},
                }
            }
        )
        self.assertEqual(ok["pointsCount"], 3)
        with self.assertRaises(QdrantApiError):
            parse_collection_info({"result": {"status": {"green": True}, "points_count": 1}})

    def test_cross_manifest_resume_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_s:
            tmp = Path(tmp_s)
            env = base_env(tmp)
            b1 = tmp / "b1"
            b2 = tmp / "b2"
            self.assertEqual(run_backup(env, b1).returncode, 0)
            self.assertEqual(run_backup(env, b2).returncode, 0)
            env["MARKHAND_RESTORE_CONFIRM"] = "I_UNDERSTAND_DESTRUCTIVE_RESTORE"
            target = tmp / "target"
            self.assertEqual(
                run(
                    [
                        "python3",
                        str(PIPELINE),
                        "restore",
                        "--backup-root",
                        str(b1),
                        "--target-state",
                        str(target),
                        "--apply",
                    ],
                    env,
                ).returncode,
                0,
            )
            # Reset stage but keep campaign identity — different manifest must fail.
            (target / "stage").write_text("none\n")
            shutil.rmtree(target / "shadow-pgdata", ignore_errors=True)
            for name in (
                "postgres-postcheck.json",
                "pg-recovery-state.json",
                "minio-version-mapping.json",
                "qdrant-shadow.json",
                "cutover-receipt.json",
                "reconcile.json",
            ):
                (target / name).unlink(missing_ok=True)
            bad = run(
                [
                    "python3",
                    str(PIPELINE),
                    "restore",
                    "--backup-root",
                    str(b2),
                    "--target-state",
                    str(target),
                    "--apply",
                ],
                env,
            )
            self.assertNotEqual(bad.returncode, 0)
            self.assertIn("campaign identity mismatch", bad.stderr)

    def test_minio_traversal_key_rejected(self) -> None:
        sys.path.insert(0, str(BACKUP / "lib"))
        from minio_http import MinioHttpError, validate_object_key  # type: ignore

        with self.assertRaises(MinioHttpError):
            validate_object_key("../etc/passwd")
        with self.assertRaises(MinioHttpError):
            validate_object_key("/abs")

    def test_streaming_crypto_large_chunked(self) -> None:
        sys.path.insert(0, str(BACKUP / "lib"))
        from crypto import CryptoError, decrypt_file, encrypt_file  # type: ignore

        with tempfile.TemporaryDirectory() as tmp_s:
            tmp = Path(tmp_s)
            plain = tmp / "big.bin"
            # >1MiB to force chunked streaming
            plain.write_bytes(os.urandom(1024 * 1024 + 1234))
            os.environ["MARKHAND_BACKUP_PG_ENCRYPTION_KEY"] = PG_KEY
            meta = encrypt_file(
                plain,
                tmp / "big.enc",
                key_env="MARKHAND_BACKUP_PG_ENCRYPTION_KEY",
                key_id="pg-enc-1",
                aad=b"chunk-aad",
            )
            decrypt_file(
                tmp / "big.enc",
                tmp / "out.bin",
                key_env="MARKHAND_BACKUP_PG_ENCRYPTION_KEY",
                meta=meta,
                aad=b"chunk-aad",
                expected_key_id="pg-enc-1",
            )
            self.assertEqual(plain.read_bytes(), (tmp / "out.bin").read_bytes())
            ct = bytearray((tmp / "big.enc").read_bytes())
            ct[100] ^= 1
            (tmp / "tamper.enc").write_bytes(ct)
            with self.assertRaises(CryptoError):
                decrypt_file(
                    tmp / "tamper.enc",
                    tmp / "x.bin",
                    key_env="MARKHAND_BACKUP_PG_ENCRYPTION_KEY",
                    meta={**meta, "ciphertextSha256": hashlib.sha256(ct).hexdigest()},
                    aad=b"chunk-aad",
                    expected_key_id="pg-enc-1",
                )
            os.environ["MARKHAND_BACKUP_PG_ENCRYPTION_KEY"] = "11" * 32
            with self.assertRaises(CryptoError):
                decrypt_file(
                    tmp / "big.enc",
                    tmp / "wk.bin",
                    key_env="MARKHAND_BACKUP_PG_ENCRYPTION_KEY",
                    meta=meta,
                    aad=b"chunk-aad",
                    expected_key_id="pg-enc-1",
                )
            os.environ["MARKHAND_BACKUP_PG_ENCRYPTION_KEY"] = PG_KEY
            trunc = (tmp / "big.enc").read_bytes()[:32]
            (tmp / "trunc.enc").write_bytes(trunc)
            with self.assertRaises(CryptoError):
                decrypt_file(
                    tmp / "trunc.enc",
                    tmp / "t.bin",
                    key_env="MARKHAND_BACKUP_PG_ENCRYPTION_KEY",
                    meta={**meta, "ciphertextSha256": hashlib.sha256(trunc).hexdigest()},
                    aad=b"chunk-aad",
                    expected_key_id="pg-enc-1",
                )

    def test_drift_repair_and_ready_false(self) -> None:
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
            env["MARKHAND_FAKE_PSQL_STATE"] = str(tmp / "psql-repair")
            env["MARKHAND_FAKE_RECONCILE_RESULT"] = "drift"
            env["MARKHAND_FAKE_RECONCILE_REPAIR"] = "1"
            ok = run(
                [
                    "python3",
                    str(PIPELINE),
                    "restore",
                    "--backup-root",
                    str(backup_dir),
                    "--target-state",
                    str(tmp / "t-repair"),
                    "--apply",
                ],
                env,
            )
            self.assertEqual(ok.returncode, 0, ok.stderr)
            recon = json.loads((tmp / "t-repair" / "reconcile.json").read_text())
            self.assertTrue(recon["ready"])

    def test_fence_ordered_bounded_opt_in_and_restart_receipt(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_s:
            tmp = Path(tmp_s)
            env = base_env(tmp)
            # Missing opt-in must fail when docker absent.
            env.pop("MARKHAND_BACKUP_ALLOW_ORDERED_BOUNDED", None)
            out = tmp / "fence.json"
            denied = run(
                ["python3", str(PIPELINE), "fence", "--output", str(out)],
                env,
            )
            self.assertNotEqual(denied.returncode, 0)
            env["MARKHAND_BACKUP_ALLOW_ORDERED_BOUNDED"] = "1"
            env["MARKHAND_BACKUP_ORDERED_BOUNDED_MAX_SECS"] = "120"
            ok = run(
                ["python3", str(PIPELINE), "fence", "--output", str(out)],
                env,
            )
            self.assertEqual(ok.returncode, 0, ok.stderr)
            fence = json.loads(out.read_text())
            self.assertEqual(fence["mode"], "ordered-bounded")
            self.assertFalse(fence["writesFenced"])
            self.assertFalse(fence["claimsStrictConsistency"])
            self.assertIn("measuredDurationSecs", fence)

    def test_tls_and_credential_non_argv(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_s:
            tmp = Path(tmp_s)
            env = base_env(tmp)
            # Live-like without insecure flags must require https/sslmode.
            env.pop("MARKHAND_BACKUP_ALLOW_INSECURE_HTTP", None)
            env.pop("MARKHAND_BACKUP_ALLOW_INSECURE_PG", None)
            bad = run_backup(env, tmp / "backup")
            self.assertNotEqual(bad.returncode, 0)
            # mc alias set with secrets must be rejected by fake.
            mc = run(
                [
                    "mc",
                    "alias",
                    "set",
                    "x",
                    "http://127.0.0.1",
                    "ak",
                    "backup-secret-not-logged",
                ],
                env,
            )
            self.assertNotEqual(mc.returncode, 0)

    def test_pg_wal_parser_unit(self) -> None:
        sys.path.insert(0, str(BACKUP / "lib"))
        from pg_wal import PgWalError, parse_backup_label, parse_backup_manifest  # type: ignore

        label = parse_backup_label(
            "START WAL LOCATION: 0/1600000 (file 000000010000000000000001)\n"
            "CHECKPOINT LOCATION: 0/1600000\n"
            "START TIMELINE: 1\n"
            "STOP WAL LOCATION: 0/16B3740 (file 000000010000000000000001)\n"
        )
        self.assertEqual(label.timeline_id, 1)
        with self.assertRaises(PgWalError):
            parse_backup_label("START WAL LOCATION: 0/1\nSTART TIMELINE: 1\n")
        ranges = parse_backup_manifest(
            json.dumps(
                {
                    "PostgreSQL-Backup-Manifest-Version": 2,
                    "WAL-Ranges": [
                        {"Timeline": 1, "Start-LSN": "0/1600000", "End-LSN": "0/16B3740"}
                    ],
                }
            )
        )
        self.assertEqual(len(ranges), 1)

    def test_nan_rejected(self) -> None:
        sys.path.insert(0, str(BACKUP / "lib"))
        from strictjson import StrictJsonError, loads  # type: ignore

        with self.assertRaises(StrictJsonError):
            loads('{"a":NaN}')
        with self.assertRaises(StrictJsonError):
            loads('{"a":Infinity}')

    def test_migration_lexer_literals(self) -> None:
        result = run(
            [sys.executable, str(MIGRATION_SAFETY), "--self-test"],
            os.environ.copy(),
        )
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)

    def test_schema_unknown_and_drift(self) -> None:
        sys.path.insert(0, str(BACKUP / "lib"))
        from manifest import ManifestError, assert_schema_code_agreement  # type: ignore
        from schema_validate import SchemaError, load_schema, validate_manifest  # type: ignore

        assert_schema_code_agreement()
        with self.assertRaises(SchemaError):
            validate_manifest({"schemaVersion": 1, "unexpected": 1})
        schema = load_schema()
        schema["definitions"]["postgres"]["properties"]["method"]["enum"] = ["pg_basebackup_pitr"]
        with self.assertRaises(ManifestError):
            assert_schema_code_agreement(schema)

    def test_pitr_blocked_without_packaged_archive(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_s:
            tmp = Path(tmp_s)
            env = base_env(tmp)
            env["MARKHAND_BACKUP_PITR_ARCHIVE"] = "1"
            env["MARKHAND_PG_ARCHIVE_MODE"] = "on"
            result = run_backup(env, tmp / "backup")
            self.assertNotEqual(result.returncode, 0)

    def test_destructive_confirm_required(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_s:
            tmp = Path(tmp_s)
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


def build_evidence(test_result: unittest.TestResult, static_errors: list[str]) -> dict[str, Any]:
    return {
        "version": 3,
        "issue": "P1B-O03",
        "status": "in_progress",
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
        "encryption": "aes-256-ctr-hmac-sha256-v1 streaming (libcrypto AES-CTR + stdlib HKDF/HMAC; not GCM/AEAD)",
        "evidenceClasses": {
            "implemented": [
                "PG18 backup_label/backup_manifest WAL-Ranges + junk rejection",
                "shadow recovery configure+verify via pinned pg_ctl/docker path",
                "campaign identity + atomic checkpoints + cutover receipts from ops",
                "MinIO encrypted opaque object bodies; keys not used as paths",
                "Qdrant v1.18.2 schema parse + alias cutover after verify",
                "streaming EtM crypto; readiness sealed campaign; fence opt-in",
                "migration base-ref + SQL lexer; JSON NaN reject; appVersion range",
            ],
            "static": [
                "digest pins",
                "runbooks",
                "migration safety + base-ref anchor",
                "wal-archive overlay preparatory only",
            ],
            "contract": [
                "stateful fake CLI/HTTP adapters (no hermetic shortcuts)",
                "junk WAL / Qdrant schema / cross-manifest resume",
                "MinIO traversal + encrypted bodies",
                "TLS/credential non-argv",
                "apply success + drift/repair + stage failure paths",
            ],
            "pending_live": [
                "Docker compose restore with measured RPO/RTO",
                "continuous PITR only after packaged archive WAL + restore consume",
            ],
        },
        "remainingBlockers": [
            "No Docker daemon in this environment for live compose cutover drill",
            "Profile-B RPO≤15m / RTO gates unresolved",
            "Live MinIO/Qdrant/PG with real TLS certs not exercised here",
        ],
        "hermeticTestsRun": test_result.testsRun,
        "hermeticFailures": [
            str(item[0]) for item in list(test_result.failures) + list(test_result.errors)
        ],
        "staticErrors": static_errors,
        "commands": [
            "python3 scripts/check-backup-o03.py --self-test",
            "make check-migrations",
            "make check-backup",
            "cargo test -p fileconv-server reconcile_report_drift_blocks_zero_drift_label",
        ],
    }


def write_report_md(evidence: dict[str, Any]) -> None:
    lines = [
        "# P1B-O03 evidence — backup/restore and migration safety",
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
    lines.extend(["## Remaining blockers", ""])
    for item in evidence.get("remainingBlockers") or []:
        lines.append(f"- {item}")
    lines.append("")
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
        f"(tests={result.testsRun}, static_errors={len(checks.errors)}, "
        f"claims_live_restore=false, claims_rpo_rto_pass=false, status=in_progress)"
    )
    for error in checks.errors:
        print(f"- {error}", file=sys.stderr)
    return 0 if evidence["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
