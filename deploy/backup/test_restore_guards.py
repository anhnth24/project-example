#!/usr/bin/env python3
"""Hermetic O03 manifest/restore guards (schema, symlink, secrets, promote)."""

from __future__ import annotations

import hashlib
import hmac
import json
import os
import stat
import subprocess
import tempfile
import unittest
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
LIB = ROOT / "deploy" / "backup" / "lib"
RESTORE = ROOT / "deploy" / "backup" / "restore.sh"
KEY = "k" * 32
KEY_ID = "o03-test-key"


def _sign(raw: bytes) -> str:
    return hmac.new(KEY.encode(), raw, hashlib.sha256).hexdigest()


def write_backup(tmp: Path, *, mutate: str | None = None) -> Path:
    import sys

    sys.path.insert(0, str(LIB))
    from manifest import SCHEMA_VERSION, canonical_dumps

    epoch = str(uuid.uuid4())
    obj = b"OBJECT-BYTES"
    digest = hashlib.sha256(obj).hexdigest()
    (tmp / "objects").mkdir(parents=True, exist_ok=True)
    (tmp / "objects" / "obj1.bin").write_bytes(obj)
    files = {
        "postgres.dump": b"PGDUMP",
        "minio-versions.txt": b"",
        "minio-versions.jsonl": b"{}\n",
        "minio-object-checksums.json": (
            json.dumps(
                {
                    "bundled": True,
                    "objects": [
                        {
                            "key": "trusted/a",
                            "versionId": "v1",
                            "objectSha256": digest,
                            "byteLength": len(obj),
                            "bundleFile": "objects/obj1.bin",
                        }
                    ],
                }
            )
            + "\n"
        ).encode(),
        "minio-tombstones.json": b'{"tombstones":[]}\n',
        "minio-normalized-history.json": b'{"keys":[]}\n',
        "qdrant-snapshot-create.json": b'{"ok":true,"result":{"name":"s1"}}\n',
        "qdrant-snapshot.bin": b"QDRANT-BYTES",
        "qdrant-snapshot.name": b"s1\n",
        "WRITE_FENCE": b"fence\n",
        "fence-epoch.txt": (epoch + "\n").encode(),
        "capture-start.epoch": b"100\n",
        "capture-end.epoch": b"110\n",
        "objects/obj1.bin": obj,
    }
    checksums = {}
    sizes = {}
    for name, data in files.items():
        path = tmp / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        checksums[name] = hashlib.sha256(data).hexdigest()
        sizes[name] = len(data)
    payload = {
        "schemaVersion": SCHEMA_VERSION,
        "capturedAt": "20260723T000000Z",
        "fenceEpoch": epoch,
        "fenceSetAt": "2026-07-23T00:00:00+00",
        "captureStartEpoch": 100,
        "captureEndEpoch": 110,
        "appVersion": "test",
        "migrationVersion": "0028_expand_audit_ownership_migrator.sql",
        "mode": "blue_green",
        "fence": "WRITE_FENCE",
        "opsFence": "restore",
        "opsFenceMandatory": True,
        "sourceIds": {
            "pgSystemIdentifier": "1",
            "pgDatabase": "markhand",
            "minioEndpoint": "http://127.0.0.1:9010",
            "minioBucket": "markhand-documents",
            "qdrantUrl": "http://127.0.0.1:6343",
            "qdrantCollection": "markhand-blue",
        },
        "postgres": {
            "dump": "postgres.dump",
            "migrations": ["0028_expand_audit_ownership_migrator.sql"],
            "systemIdentifier": "1",
            "database": "markhand",
        },
        "minio": {
            "versioning": "Enabled",
            "inventory": "minio-versions.jsonl",
            "objects": "minio-object-checksums.json",
            "tombstones": "minio-tombstones.json",
        },
        "qdrant": {
            "snapshot": "qdrant-snapshot.bin",
            "collection": "markhand-blue",
            "pointsCount": 1,
            "configSha256": "a" * 64,
            "payloadRefSha256": "b" * 64,
        },
        "artifactSha256": checksums,
        "artifactBytes": sizes,
        "crossStoreRefs": [],
        "watermarks": {
            "pgWalLsn": "0/1",
            "fenceEpoch": epoch,
            "jobsDrained": True,
            "writeGate": "fence_drain_lock_app_write_gate_absent",
        },
        "trustedBoundary": {
            "mode": "hmac_sha256",
            "keyId": KEY_ID,
            "signatureFile": "manifest.sig",
            "note": "test",
        },
        "rpoSecondsTarget": 900,
        "queryReadyRtoSecondsTarget": 3600,
        "status": "captured",
    }
    if mutate == "downgrade":
        payload["schemaVersion"] = 2
    if mutate == "traversal":
        payload["artifactSha256"]["../evil"] = "c" * 64
        payload["artifactBytes"]["../evil"] = 1
    if mutate == "additional":
        payload["unexpectedField"] = "nope"
    raw = canonical_dumps(payload)
    if mutate == "tamper-body":
        (tmp / "manifest.sig").write_text(_sign(raw) + "\n", encoding="utf-8")
        (tmp / "manifest.json").write_bytes(raw[:-2] + b"x\n")
        return tmp
    if mutate == "malformed":
        (tmp / "manifest.json").write_bytes(b"{not-json")
        (tmp / "manifest.sig").write_text(_sign(b"{not-json") + "\n", encoding="utf-8")
        return tmp
    (tmp / "manifest.json").write_bytes(raw)
    (tmp / "manifest.sig").write_text(_sign(raw) + "\n", encoding="utf-8")
    (tmp / "manifest.sha256").write_text(
        hashlib.sha256(raw).hexdigest() + "\n", encoding="utf-8"
    )
    return tmp


class ManifestGuardTests(unittest.TestCase):
    def setUp(self) -> None:
        os.environ["MARKHAND_BACKUP_SIGNING_KEY"] = KEY
        os.environ["MARKHAND_BACKUP_KEY_ID"] = KEY_ID

    def test_auth_before_parse_rejects_bad_sig(self) -> None:
        import sys

        sys.path.insert(0, str(LIB))
        from manifest import ManifestError, load_authenticated_manifest

        with tempfile.TemporaryDirectory() as td:
            backup = write_backup(Path(td))
            (backup / "manifest.sig").write_text("0" * 64 + "\n", encoding="utf-8")
            with self.assertRaises(ManifestError):
                load_authenticated_manifest(backup)

    def test_downgrade_schema_refused(self) -> None:
        import sys

        sys.path.insert(0, str(LIB))
        from manifest import ManifestError, load_authenticated_manifest

        with tempfile.TemporaryDirectory() as td:
            backup = write_backup(Path(td), mutate="downgrade")
            with self.assertRaises(ManifestError) as ctx:
                load_authenticated_manifest(backup)
            self.assertIn("schema", str(ctx.exception).lower())

    def test_additional_properties_refused(self) -> None:
        import sys

        sys.path.insert(0, str(LIB))
        from manifest import ManifestError, load_authenticated_manifest

        with tempfile.TemporaryDirectory() as td:
            backup = write_backup(Path(td), mutate="additional")
            with self.assertRaises(ManifestError) as ctx:
                load_authenticated_manifest(backup)
            self.assertIn("additional", str(ctx.exception).lower())

    def test_traversal_path_refused(self) -> None:
        import sys

        sys.path.insert(0, str(LIB))
        from manifest import ManifestError, load_authenticated_manifest

        with tempfile.TemporaryDirectory() as td:
            backup = write_backup(Path(td), mutate="traversal")
            with self.assertRaises(ManifestError):
                load_authenticated_manifest(backup)

    def test_symlink_artifact_refused(self) -> None:
        import sys

        sys.path.insert(0, str(LIB))
        from manifest import ManifestError, verify_artifacts, load_authenticated_manifest

        with tempfile.TemporaryDirectory() as td:
            backup = write_backup(Path(td))
            manifest, _ = load_authenticated_manifest(backup)
            target = backup / "postgres.dump"
            link = backup / "objects" / "obj1.bin"
            link.unlink()
            link.symlink_to(target)
            with self.assertRaises(ManifestError) as ctx:
                verify_artifacts(backup, manifest)
            self.assertIn("symlink", str(ctx.exception).lower())

    def test_malformed_after_auth_refused(self) -> None:
        import sys

        sys.path.insert(0, str(LIB))
        from manifest import ManifestError, load_authenticated_manifest

        with tempfile.TemporaryDirectory() as td:
            backup = write_backup(Path(td), mutate="malformed")
            with self.assertRaises(ManifestError):
                load_authenticated_manifest(backup)

    def test_tampered_body_refused(self) -> None:
        import sys

        sys.path.insert(0, str(LIB))
        from manifest import ManifestError, load_authenticated_manifest

        with tempfile.TemporaryDirectory() as td:
            backup = write_backup(Path(td), mutate="tamper-body")
            with self.assertRaises(ManifestError):
                load_authenticated_manifest(backup)

    def test_missing_green_targets_refuses_destructive(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            backup = write_backup(Path(td))
            proc = subprocess.run(
                ["bash", str(RESTORE), str(backup)],
                env={
                    "PATH": "/usr/bin:/bin",
                    "DATABASE_URL": "postgres://unused",
                    "MARKHAND_BACKUP_SIGNING_KEY": KEY,
                    "MARKHAND_BACKUP_KEY_ID": KEY_ID,
                },
                capture_output=True,
                text=True,
            )
            self.assertEqual(proc.returncode, 2)
            self.assertIn("REFUSING_DESTRUCTIVE_PROMOTE", proc.stderr + proc.stdout)

    def test_promote_flag_disabled(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            backup = write_backup(Path(td))
            env = {
                "PATH": "/usr/bin:/bin:/usr/local/bin",
                "DATABASE_URL": "postgres://u@127.0.0.1:1/db",
                "MARKHAND_BACKUP_SIGNING_KEY": KEY,
                "MARKHAND_BACKUP_KEY_ID": KEY_ID,
                "MARKHAND_GREEN_DATABASE_URL": "postgres://u@127.0.0.1:1/green",
                "MARKHAND_GREEN_MINIO_BUCKET": "green-bucket",
                "MARKHAND_GREEN_QDRANT_COLLECTION": "green-coll",
                "MARKHAND_GREEN_ALLOWLIST_JSON": "[]",
                "MARKHAND_RESTORE_CUTOVER": "1",
                "PYTHONPATH": str(LIB),
            }
            # Will fail during restore-green (no live PG) OR after with promote disabled.
            proc = subprocess.run(
                ["bash", str(RESTORE), str(backup)],
                env=env,
                capture_output=True,
                text=True,
            )
            combined = proc.stderr + proc.stdout
            # Either preflight fails closed, or promote disable triggers.
            self.assertNotEqual(proc.returncode, 0)
            self.assertTrue(
                "PROMOTE_DISABLED" in combined
                or "pipeline_error" in combined
                or "allowlist" in combined.lower()
                or "REFUSING" in combined
            )

    def test_key_required(self) -> None:
        import sys

        sys.path.insert(0, str(LIB))
        from manifest import ManifestError, require_signing_env

        os.environ.pop("MARKHAND_BACKUP_SIGNING_KEY", None)
        os.environ.pop("MARKHAND_BACKUP_KEY_ID", None)
        with self.assertRaises(ManifestError):
            require_signing_env()

    def test_encryption_policy_fail_closed(self) -> None:
        import sys

        sys.path.insert(0, str(LIB))
        from pipeline import PipelineError, assert_encryption_policy

        os.environ.pop("MARKHAND_BACKUP_ENCRYPTED", None)
        os.environ.pop("MARKHAND_BACKUP_UNENCRYPTED_DEST_POLICY", None)
        with self.assertRaises(PipelineError):
            assert_encryption_policy(Path("/tmp/markhand-backup/x"))
        os.environ["MARKHAND_BACKUP_UNENCRYPTED_DEST_POLICY"] = "explicit_poc_tmp_only"
        # Should not raise for tmp path.
        assert_encryption_policy(Path("/tmp/markhand-backup/x"))

    def test_no_password_on_argv_helper(self) -> None:
        import sys

        sys.path.insert(0, str(LIB))
        from pg_session import PgSessionError, assert_no_password_argv

        with self.assertRaises(PgSessionError):
            assert_no_password_argv(
                ["psql", "postgres://user:secretpass@127.0.0.1:5432/db", "-c", "select 1"]
            )

    def test_safe_rel_is_relative_to(self) -> None:
        import sys

        sys.path.insert(0, str(LIB))
        from manifest import ManifestError, safe_open_under

        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            (root / "ok.txt").write_text("x", encoding="utf-8")
            p = safe_open_under(root, "ok.txt")
            self.assertTrue(p.is_relative_to(root.resolve()))
            with self.assertRaises(ManifestError):
                safe_open_under(root, "../escape")

    def test_write_signed_manifest_mode_0600(self) -> None:
        import sys

        sys.path.insert(0, str(LIB))
        from manifest import load_authenticated_manifest, write_signed_manifest

        with tempfile.TemporaryDirectory() as td:
            backup = write_backup(Path(td))
            manifest, _ = load_authenticated_manifest(backup)
            write_signed_manifest(backup, manifest)
            mode = stat.S_IMODE((backup / "manifest.json").stat().st_mode)
            self.assertEqual(mode, 0o600)

    def test_endpoint_alias_detection(self) -> None:
        import sys

        sys.path.insert(0, str(LIB))
        from targets import TargetError, assert_not_blue_alias, endpoint_alias

        self.assertTrue(endpoint_alias("http://127.0.0.1:9010", "http://localhost:9010"))
        with self.assertRaises(TargetError):
            assert_not_blue_alias(
                blue_bucket="b",
                green_bucket="b",
                blue_collection="c1",
                green_collection="c2",
                blue_endpoint="http://127.0.0.1:9010",
                green_endpoint="http://127.0.0.1:9010",
                blue_qdrant="http://127.0.0.1:6343",
                green_qdrant="http://127.0.0.1:6343",
            )

    def test_mandatory_minio_qdrant_allowlists(self) -> None:
        import sys

        sys.path.insert(0, str(LIB))
        from targets import GreenAllowlists, TargetError

        os.environ["MARKHAND_GREEN_ALLOWLIST_JSON"] = json.dumps(
            [{"pgSystemIdentifier": "1", "pgDatabase": "g"}]
        )
        os.environ.pop("MARKHAND_GREEN_MINIO_ALLOWLIST_JSON", None)
        os.environ.pop("MARKHAND_GREEN_QDRANT_ALLOWLIST_JSON", None)
        with self.assertRaises(TargetError):
            GreenAllowlists.load_from_env()
        os.environ["MARKHAND_GREEN_MINIO_ALLOWLIST_JSON"] = json.dumps(["green-bucket"])
        with self.assertRaises(TargetError):
            GreenAllowlists.load_from_env()
        os.environ["MARKHAND_GREEN_QDRANT_ALLOWLIST_JSON"] = json.dumps(["green-coll"])
        al = GreenAllowlists.load_from_env()
        self.assertEqual(al.minio_buckets, ("green-bucket",))

    def test_private_pgpass_never_alters_caller(self) -> None:
        import sys

        sys.path.insert(0, str(LIB))
        from pg_session import private_pg_env

        os.environ["PGPASSFILE"] = "/tmp/caller-pgpass-should-remain"
        caller = os.environ["PGPASSFILE"]
        with private_pg_env("postgres://u:secretpass@127.0.0.1:5432/db") as (safe, env):
            self.assertNotIn("secretpass", safe)
            self.assertNotEqual(env.get("PGPASSFILE"), caller)
            self.assertEqual(os.environ.get("PGPASSFILE"), caller)
            self.assertTrue(Path(env["PGPASSFILE"]).is_file())
            private_path = env["PGPASSFILE"]
        self.assertEqual(os.environ.get("PGPASSFILE"), caller)
        self.assertFalse(Path(private_path).exists())

    def test_mc_credentials_argv_refused(self) -> None:
        import sys

        sys.path.insert(0, str(LIB))
        from pg_session import PgSessionError, assert_no_mc_credentials_argv

        with self.assertRaises(PgSessionError):
            assert_no_mc_credentials_argv(
                ["mc", "alias", "set", "local", "http://127.0.0.1:9010", "ak", "sk"]
            )
        with self.assertRaises(PgSessionError):
            assert_no_mc_credentials_argv(
                ["mc", "ls", "http://ak:sk@127.0.0.1:9010/bucket"]
            )

    def test_normalized_history_compare_missing_intermediate(self) -> None:
        import sys

        sys.path.insert(0, str(LIB))
        from pipeline import PipelineError, compare_normalized_history

        exp = [
            {
                "key": "k",
                "events": [
                    {"type": "put", "size": 1, "contentSha256": "a" * 64},
                    {"type": "put", "size": 2, "contentSha256": "b" * 64},
                    {"type": "delete", "size": None, "contentSha256": None},
                ],
            }
        ]
        act = [
            {
                "key": "k",
                "events": [{"type": "put", "size": 2, "contentSha256": "b" * 64}],
            }
        ]
        with self.assertRaises(PipelineError) as ctx:
            compare_normalized_history(exp, act)
        self.assertIn("mismatch", str(ctx.exception).lower())

    def test_app_mutation_write_gate_is_integrated(self) -> None:
        """Central write-gate contract must be complete; negatives prove each part."""
        import shutil
        import sys
        import tempfile
        from pathlib import Path
        from unittest import mock

        sys.path.insert(0, str(LIB))
        from pipeline import (
            PipelineError,
            app_mutation_write_gate_sufficient,
            assert_consistency_write_gate,
        )
        from write_gate_contract import (
            app_mutation_write_gate_sufficient_in,
            evaluate_write_gate_tree,
        )

        server_src = Path("/workspace/crates/server/src")
        self.assertTrue(
            app_mutation_write_gate_sufficient(),
            f"missing write-gate contract parts: {evaluate_write_gate_tree(server_src)}",
        )
        os.environ["MARKHAND_BACKUP_REQUIRE_APP_WRITE_GATE"] = "1"
        self.assertEqual(
            assert_consistency_write_gate(),
            "app_mutation_write_gate+ops_fences.restore",
        )
        with mock.patch(
            "pipeline.app_mutation_write_gate_sufficient", return_value=False
        ):
            with self.assertRaises(PipelineError) as ctx:
                assert_consistency_write_gate()
            self.assertIn(
                "REFUSING_CONSISTENCY_BACKUP_WRITE_GATE_UNAVAILABLE",
                str(ctx.exception),
            )
        os.environ["MARKHAND_BACKUP_REQUIRE_APP_WRITE_GATE"] = "0"
        with mock.patch(
            "pipeline.app_mutation_write_gate_sufficient", return_value=False
        ):
            self.assertEqual(
                assert_consistency_write_gate(),
                "fence_drain_lock_app_write_gate_absent",
            )

        # Negative fixtures: remove each required component → False.
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            shutil.copytree(server_src / "middleware", root / "middleware")
            (root / "http.rs").write_text(
                (server_src / "http.rs").read_text(encoding="utf-8"),
                encoding="utf-8",
            )
            self.assertTrue(app_mutation_write_gate_sufficient_in(root))

            cases = {
                "middleware_fn": lambda: (root / "middleware" / "write_gate.rs").write_text(
                    (root / "middleware" / "write_gate.rs")
                    .read_text(encoding="utf-8")
                    .replace("pub async fn mutation_write_gate", "async fn mutation_write_gate_x"),
                    encoding="utf-8",
                ),
                "lock_key": lambda: (root / "middleware" / "write_gate.rs").write_text(
                    (root / "middleware" / "write_gate.rs")
                    .read_text(encoding="utf-8")
                    .replace("7303003", "9999999"),
                    encoding="utf-8",
                ),
                "router_wired": lambda: (root / "http.rs").write_text(
                    (root / "http.rs")
                    .read_text(encoding="utf-8")
                    .replace(
                        "from_fn_with_state(state.clone(), mutation_write_gate)",
                        "from_fn_with_state(state.clone(), baseline_ip_rate_limit)",
                    ),
                    encoding="utf-8",
                ),
                "background_skip": lambda: (root / "http.rs").write_text(
                    (root / "http.rs")
                    .read_text(encoding="utf-8")
                    .replace(
                        "ensure_background_mutations_allowed(&pool)",
                        "Ok::<(), ()>(())",
                    ),
                    encoding="utf-8",
                ),
                "middleware_absent": lambda: shutil.rmtree(root / "middleware"),
            }
            # Re-copy fresh tree per negative case.
            for name, mutate in cases.items():
                shutil.rmtree(root)
                shutil.copytree(server_src / "middleware", root / "middleware")
                (root / "http.rs").write_text(
                    (server_src / "http.rs").read_text(encoding="utf-8"),
                    encoding="utf-8",
                )
                mutate()
                self.assertFalse(
                    app_mutation_write_gate_sufficient_in(root),
                    f"negative fixture {name} must fail contract",
                )


if __name__ == "__main__":
    unittest.main()
