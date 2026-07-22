#!/usr/bin/env python3
"""Hermetic corrupt/missing/blue-green restore guards (no live MinIO/Qdrant)."""

from __future__ import annotations

import hashlib
import json
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
RESTORE = ROOT / "deploy" / "backup" / "restore.sh"
BACKUP = ROOT / "deploy" / "backup" / "backup.sh"


def write_backup(tmp: Path, *, corrupt: str | None = None, missing: str | None = None) -> Path:
    tmp.mkdir(parents=True, exist_ok=True)
    obj_dir = tmp / "objects"
    obj_dir.mkdir(exist_ok=True)
    payload = b"OBJECT-BYTES"
    digest = hashlib.sha256(payload).hexdigest()
    bundle = obj_dir / "obj1.bin"
    bundle.write_bytes(payload)
    artifacts = {
        "postgres.dump": b"PGDUMP",
        "minio-versions.txt": b"",
        "minio-versions.jsonl": b"",
        "minio-object-checksums.json": (
            json.dumps(
                {
                    "bundled": True,
                    "objects": [
                        {
                            "key": "trusted/a",
                            "versionId": "v1",
                            "objectSha256": digest,
                            "byteLength": len(payload),
                            "bundleFile": "objects/obj1.bin",
                        }
                    ],
                },
                indent=2,
            )
            + "\n"
        ).encode(),
        "qdrant-snapshot-create.json": b'{"ok":true,"result":{"name":"s1"}}\n',
        "qdrant-snapshot.bin": b"QDRANT-BYTES",
        "qdrant-snapshot.name": b"s1\n",
        "WRITE_FENCE": b"fence\n",
        "objects/obj1.bin": payload,
    }
    if missing:
        artifacts.pop(missing, None)
    checksums = {}
    for name, data in artifacts.items():
        path = tmp / name
        path.parent.mkdir(parents=True, exist_ok=True)
        body = b"CORRUPT" if corrupt == name else data
        path.write_bytes(body)
        checksums[name] = hashlib.sha256(data).hexdigest()
    manifest = {
        "mode": "blue_green",
        "opsFenceMandatory": True,
        "artifactSha256": checksums,
        "stores": {
            "postgres": "postgres.dump",
            "minioObjectChecksums": "minio-object-checksums.json",
            "minioObjectBytes": "objects/",
            "qdrantSnapshotBytes": "qdrant-snapshot.bin",
        },
        "status": "captured",
    }
    body = (json.dumps(manifest, indent=2) + "\n").encode()
    (tmp / "manifest.json").write_bytes(body)
    (tmp / "manifest.sha256").write_text(
        hashlib.sha256(body).hexdigest() + "\n", encoding="utf-8"
    )
    return tmp


class RestoreGuardTests(unittest.TestCase):
    def test_corrupt_artifact_refused(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            backup = write_backup(Path(td), corrupt="qdrant-snapshot.bin")
            proc = subprocess.run(
                ["bash", str(RESTORE), str(backup)],
                env={"PATH": "/usr/bin:/bin", "DATABASE_URL": "postgres://unused"},
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(proc.returncode, 0)
            self.assertIn("checksum mismatch", (proc.stderr + proc.stdout).lower())

    def test_missing_artifact_refused(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            backup = write_backup(Path(td))
            (backup / "postgres.dump").unlink()
            proc = subprocess.run(
                ["bash", str(RESTORE), str(backup)],
                env={"PATH": "/usr/bin:/bin", "DATABASE_URL": "postgres://unused"},
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(proc.returncode, 0)
            combined = (proc.stderr + proc.stdout).lower()
            self.assertTrue("missing" in combined or "checksum" in combined)

    def test_manifest_digest_mismatch_refused(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            backup = write_backup(Path(td))
            (backup / "manifest.sha256").write_text("0" * 64 + "\n", encoding="utf-8")
            proc = subprocess.run(
                ["bash", str(RESTORE), str(backup)],
                env={"PATH": "/usr/bin:/bin", "DATABASE_URL": "postgres://unused"},
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(proc.returncode, 0)
            self.assertIn("manifest checksum mismatch", (proc.stderr + proc.stdout).lower())

    def test_missing_green_targets_refuses_destructive(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            backup = write_backup(Path(td))
            proc = subprocess.run(
                ["bash", str(RESTORE), str(backup)],
                env={"PATH": "/usr/bin:/bin", "DATABASE_URL": "postgres://unused"},
                capture_output=True,
                text=True,
            )
            self.assertEqual(proc.returncode, 2)
            combined = proc.stderr + proc.stdout
            self.assertIn("REFUSING_DESTRUCTIVE_PROMOTE", combined)

    def test_no_env_fake_attestation_shortcuts(self) -> None:
        source = RESTORE.read_text(encoding="utf-8")
        self.assertNotIn("MARKHAND_OBJECT_RESTORE_ATTESTATION", source)
        self.assertNotIn("MARKHAND_VECTOR_RESTORE_ATTESTATION", source)
        self.assertIn("MARKHAND_GREEN_DATABASE_URL", source)
        self.assertIn("REFUSING_CUTOVER_UNTIL_RECONCILE", source)
        self.assertIn("fence clear failed", source)
        backup_src = BACKUP.read_text(encoding="utf-8")
        self.assertIn("ops_fences", backup_src)
        self.assertIn("bundled", backup_src.lower() + backup_src)

    def test_orphan_bundle_entry_refused(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            backup = write_backup(Path(td))
            (backup / "objects" / "obj1.bin").unlink()
            proc = subprocess.run(
                ["bash", str(RESTORE), str(backup)],
                env={"PATH": "/usr/bin:/bin", "DATABASE_URL": "postgres://unused"},
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(proc.returncode, 0)
            self.assertIn("missing", (proc.stderr + proc.stdout).lower())


if __name__ == "__main__":
    unittest.main()
