#!/usr/bin/env python3
"""Tests for fail-closed O03 raw-evidence report evaluation."""

from __future__ import annotations

import hashlib
import json
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
REPORTER = ROOT / "deploy" / "scripts" / "o03-report-from-raw.py"


def seed_complete_raw(raw: Path) -> None:
    raw.mkdir(parents=True)
    (raw / "passes.txt").write_text("restore complete\n", encoding="utf-8")
    (raw / "gaps.txt").write_text("", encoding="utf-8")
    values = {
        "capture-window.seconds": "5",
        "restore-green.seconds": "7",
        "consistency-rpo.seconds": "12",
        "query-ready-rto.seconds": "40",
        "full-vector-rto.seconds": "41",
        "api-ready-baseline.status": "200",
        "api-live-post-restore.status": "200",
        "api-ready-post-restore.status": "200",
        "api-ready-green-pre-attest.status": "503",
        "api-ready-blue-during-green.status": "503",
        "cleanup-verify.txt": "cleanup_verified=1",
        "encryption-policy.txt": "MARKHAND_BACKUP_ENCRYPTED=1 marker_verified=1",
    }
    for name, value in values.items():
        (raw / name).write_text(value + "\n", encoding="utf-8")
    (raw / "green-target-attestation.json").write_text(
        json.dumps(
            {
                "schemaVersion": 1,
                "kind": "markhand.green-target-state",
                "checks": {
                    "manifestAuthenticated": True,
                    "postgresConsistent": True,
                    "minioConsistent": True,
                    "qdrantConsistent": True,
                    "crossStoreRefsConsistent": True,
                    "restoreFenceMatches": True,
                },
            }
        ),
        encoding="utf-8",
    )
    (raw / "green-query-proof.json").write_text(
        json.dumps(
            {
                "loginHttp": 200,
                "askHttp": 200,
                "grounded": True,
                "expectedDocument": True,
            }
        ),
        encoding="utf-8",
    )
    (raw / "provenance.json").write_text(
        json.dumps(
            {
                "gitShaFull": "a" * 40,
                "gitDirty": False,
                "composeProject": "markhand-poc",
                "migrationManifestSha256": "b" * 64,
                "composeFileSha256": "c" * 64,
                "indexSignature": "d" * 64,
                "imageIds": {
                    service: f"sha256:{index:064x}"
                    for index, service in enumerate(
                        [
                            "api",
                            "minio",
                            "postgres",
                            "qdrant",
                            "worker-convert",
                            "worker-index",
                        ],
                        start=1,
                    )
                },
            }
        ),
        encoding="utf-8",
    )


class O03ReportTests(unittest.TestCase):
    def run_report(self, raw: Path, out: Path) -> dict:
        subprocess.run(
            ["python3", str(REPORTER), str(raw), "--out-dir", str(out)],
            check=True,
            capture_output=True,
            text=True,
        )
        return json.loads((out / "o03-restore.json").read_text(encoding="utf-8"))

    def test_complete_raw_evidence_passes_rpo_and_query_ready_rto(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            raw = root / "o03-complete"
            seed_complete_raw(raw)
            manifest_path = raw / "raw-manifest.json"
            manifest_path.write_text(
                json.dumps({"schemaVersion": 1, "files": []}) + "\n",
                encoding="utf-8",
            )
            manifest_sha = hashlib.sha256(manifest_path.read_bytes()).hexdigest()
            report = self.run_report(raw, root / "out")

        self.assertEqual(report["status"], "pass")
        self.assertTrue(report["consistencyRpoPass"])
        self.assertTrue(report["queryReadyRtoPass"])
        self.assertTrue(report["fullVectorRtoPass"])
        self.assertEqual(report["consistencyRpoSeconds"], 12)
        self.assertEqual(report["queryReadyRtoSeconds"], 40)
        self.assertEqual(report["fullVectorRtoSeconds"], 41)
        self.assertEqual(report["blockers"], [])
        self.assertEqual(
            report["rawArtifactManifest"]["sha256"],
            manifest_sha,
        )

    def test_missing_attestation_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            raw = root / "o03-missing-attestation"
            seed_complete_raw(raw)
            (raw / "green-target-attestation.json").unlink()
            report = self.run_report(raw, root / "out")

        self.assertEqual(report["status"], "in_progress")
        self.assertIn(
            "independent green attestation missing or incomplete", report["blockers"]
        )

    def test_dirty_or_missing_provenance_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            raw = root / "o03-dirty"
            seed_complete_raw(raw)
            provenance = json.loads(
                (raw / "provenance.json").read_text(encoding="utf-8")
            )
            provenance["gitDirty"] = True
            (raw / "provenance.json").write_text(
                json.dumps(provenance), encoding="utf-8"
            )
            report = self.run_report(raw, root / "out")

        self.assertEqual(report["status"], "in_progress")
        self.assertIn("source git worktree was dirty", report["blockers"])


if __name__ == "__main__":
    unittest.main()
