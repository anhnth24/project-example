#!/usr/bin/env python3
"""Unit / hermetic tests for P1B-F02 boot evidence hardening."""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "poc_f02_boot_evidence.py"
sys.path.insert(0, str(SCRIPT.parent))

import poc_f02_boot_evidence as f02  # noqa: E402


def _base_good_report() -> dict:
    services = list(f02.EXPECTED_POC_SERVICES)
    return {
        "issue": "P1B-F02",
        "stamp_utc": "20260724T000000Z",
        "generatedAt": "2026-07-24T00:00:00+00:00",
        "passed": True,
        "pass_count": 12,
        "fail_count": 0,
        "passes": ["convert network Internal=true"],
        "fails": [],
        "notes": [],
        "composeProject": "markhand-poc",
        "containerIds": {svc: f"container-{svc}" for svc in services},
        "imageIds": {svc: f"sha256:{i:064d}" for i, svc in enumerate(services)},
        "imageDigests": {
            "postgres": "postgres@sha256:" + ("a" * 64),
            "minio": "minio@sha256:" + ("b" * 64),
        },
        "gitSha": "abc1234",
        "gitShaFull": "abc1234deadbeef0000000000000000000000000",
        "dockerVersion": "24.0.0",
        "composeVersion": "2.24.0",
        "composeFileSha256": "c" * 64,
        "storageDriver": "overlay2",
        "nolimitComposeUsed": False,
        "cgroupLimitsEnforced": True,
        "standardHostQualification": True,
        "egressProbe": {
            "executed": True,
            "toolMissing": False,
            "blocked": True,
            "exitCode": 28,
            "raw": "wget: download timed out",
            "network": "markhand-poc_convert",
            "probeImage": "python:3.12.12-alpine@sha256:deadbeef",
        },
        "resourceLimits": {
            svc: {"memory": 512 * 1024 * 1024, "nanoCpus": 1_000_000_000, "pidsLimit": 256}
            for svc in ("api", "worker-convert", "worker-index", "worker-embedding")
        },
        "rawDir": "bench/markhand_web/reports/phase-1b-gate/raw/f02-abc1234",
        "redactionScan": {"passed": True, "findings": []},
    }


class PocF02EvidenceTests(unittest.TestCase):
    def test_sanitize_inspect_drops_env_secrets(self) -> None:
        raw = [
            {
                "Id": "sha256:abc",
                "Name": "/markhand-poc-api-1",
                "Config": {
                    "User": "10001:10001",
                    "Image": "markhand-api:poc",
                    "Env": [
                        "MARKHAND_AUTH_SIGNING_KEY=super-secret",
                        "POSTGRES_PASSWORD=hunter2",
                        "MINIO_ROOT_PASSWORD=minioadmin",
                    ],
                },
                "HostConfig": {
                    "ReadonlyRootfs": True,
                    "SecurityOpt": ["no-new-privileges:true"],
                    "CapDrop": ["ALL"],
                    "Memory": 536870912,
                    "NanoCpus": 1000000000,
                    "PidsLimit": 256,
                },
                "NetworkSettings": {"Networks": {"markhand-poc_private": {"IPAddress": "10.0.0.2"}}},
                "State": {
                    "Status": "running",
                    "Running": True,
                    "ExitCode": 0,
                    "Health": {"Status": "healthy"},
                },
                "Image": "sha256:" + ("d" * 64),
            }
        ]
        cleaned = f02.sanitize_inspect(raw)
        blob = json.dumps(cleaned)
        self.assertNotIn("Env", blob)
        self.assertNotIn("super-secret", blob)
        self.assertNotIn("hunter2", blob)
        self.assertNotIn("minioadmin", blob)
        self.assertEqual(cleaned[0]["Config"]["User"], "10001:10001")
        self.assertEqual(cleaned[0]["HostConfig"]["Memory"], 536870912)
        self.assertEqual(cleaned[0]["Image"], "sha256:" + ("d" * 64))

    def test_secret_bearing_inspect_rejected_by_scan(self) -> None:
        text = json.dumps(
            {
                "Config": {
                    "Env": [
                        "MARKHAND_AUTH_SIGNING_KEY=abcdef0123456789",
                        "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.aaa.bbb",
                    ]
                }
            }
        )
        findings = f02.scan_committed_text(text)
        self.assertTrue(findings)
        report = _base_good_report()
        with tempfile.TemporaryDirectory() as tmp:
            raw = Path(tmp)
            (raw / "inspect-api.json").write_text(text, encoding="utf-8")
            report["rawDir"] = str(raw)
            status, blockers = f02.evaluate_report(report, raw_root=raw)
        self.assertNotEqual(status, "pass")
        self.assertTrue(any("secret" in b or "redaction" in b for b in blockers), blockers)

    def test_missing_service_image_metadata_rejected(self) -> None:
        report = _base_good_report()
        report["imageIds"] = {"api": report["imageIds"]["api"]}
        report["containerIds"] = {"api": report["containerIds"]["api"]}
        status, blockers = f02.evaluate_report(report)
        self.assertNotEqual(status, "pass")
        self.assertTrue(
            any("missing" in b and ("image" in b or "container" in b or "service" in b) for b in blockers),
            blockers,
        )

    def test_missing_egress_execution_rejected(self) -> None:
        report = _base_good_report()
        report["egressProbe"] = {
            "executed": False,
            "toolMissing": True,
            "blocked": None,
            "raw": "curl absent — expected lean image",
        }
        status, blockers = f02.evaluate_report(report)
        self.assertNotEqual(status, "pass")
        self.assertIn("egress_not_executed", blockers)

    def test_resource_limit_zero_rejected(self) -> None:
        report = _base_good_report()
        report["resourceLimits"]["api"]["memory"] = 0
        status, blockers = f02.evaluate_report(report)
        self.assertNotEqual(status, "pass")
        self.assertTrue(any("resource_limit" in b for b in blockers), blockers)

    def test_nolimit_compose_cannot_pass(self) -> None:
        report = _base_good_report()
        report["nolimitComposeUsed"] = True
        status, blockers = f02.evaluate_report(report)
        self.assertNotEqual(status, "pass")
        self.assertIn("nolimit_compose", blockers)

    def test_vfs_storage_without_limits_rejected(self) -> None:
        report = _base_good_report()
        report["storageDriver"] = "vfs"
        report["resourceLimits"]["worker-convert"]["pidsLimit"] = 0
        status, blockers = f02.evaluate_report(report)
        self.assertNotEqual(status, "pass")

    def test_complete_fixture_accepted(self) -> None:
        report = _base_good_report()
        status, blockers = f02.evaluate_report(report)
        self.assertEqual(status, "pass", blockers)
        self.assertEqual(blockers, [])

    def test_repo_relative_raw_path(self) -> None:
        root = Path("/workspace")
        inside = root / "bench/markhand_web/reports/phase-1b-gate/raw/f02-abc"
        self.assertEqual(
            f02.repo_relative_raw_dir(inside, root),
            "bench/markhand_web/reports/phase-1b-gate/raw/f02-abc",
        )
        outside = Path("/tmp/markhand-f02-evidence")
        self.assertEqual(f02.repo_relative_raw_dir(outside, root), str(outside.resolve()))

    def test_cli_self_test_passes(self) -> None:
        # Invoked via module main --self-test after implementation lands.
        rc = f02.run_self_test()
        self.assertEqual(rc, 0)


if __name__ == "__main__":
    raise SystemExit(unittest.main())
