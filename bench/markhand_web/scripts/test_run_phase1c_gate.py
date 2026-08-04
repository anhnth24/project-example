#!/usr/bin/env python3
"""Hermetic contract tests for the Phase 1C deployed harness."""

from __future__ import annotations

import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

SCRIPT = Path(__file__).with_name("run_phase1c_gate.py")
SPEC = importlib.util.spec_from_file_location("run_phase1c_gate", SCRIPT)
gate = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(gate)

GATES_SPEC = importlib.util.spec_from_file_location(
    "check_markhand_gates", gate.ROOT / "scripts/check-markhand-gates.py"
)
GATES = importlib.util.module_from_spec(GATES_SPEC)
assert GATES_SPEC.loader is not None
GATES_SPEC.loader.exec_module(GATES)


def init_git_repo(repo: Path, *, marker: str = "fixture\n") -> str:
    subprocess.run(["git", "init"], cwd=repo, check=True, capture_output=True)
    subprocess.run(
        ["git", "config", "user.email", "phase1c-test@example.com"],
        cwd=repo,
        check=True,
        capture_output=True,
    )
    subprocess.run(
        ["git", "config", "user.name", "phase1c-test"],
        cwd=repo,
        check=True,
        capture_output=True,
    )
    (repo / "marker.txt").write_text(marker, encoding="utf-8")
    subprocess.run(["git", "add", "marker.txt"], cwd=repo, check=True, capture_output=True)
    subprocess.run(
        ["git", "commit", "-m", "phase1c fixture"],
        cwd=repo,
        check=True,
        capture_output=True,
    )
    return subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo, text=True).strip()


def evidence_payload(gate_id: str, *, scenario: str, extra: dict | None = None) -> dict:
    payload = {
        "gateId": gate_id,
        "scenario": scenario,
        "p1c8Items": list(gate.GATE_TO_P1C8[gate_id]),
        "status": "pass",
        "probe": {
            "commandExitCode": 0,
            "timedOut": False,
            "outputTruncated": False,
            "eof": True,
        },
    }
    if extra:
        payload.update(extra)
    return payload


class Phase1cHarnessContractTests(unittest.TestCase):
    def setUp(self) -> None:
        self._temp = tempfile.TemporaryDirectory()
        self.repo_root = Path(self._temp.name)
        self.markhand_root, self.repo_root = GATES.prepare_phase1c_fixture(self.repo_root)
        self.git_commit = init_git_repo(self.repo_root)

    def tearDown(self) -> None:
        self._temp.cleanup()

    def _load_template(self) -> dict:
        return GATES.load_json_yaml(
            self.markhand_root / "reports/phase-1c-gate/phase-1c-gate.template.json"
        )

    def _metric_value(self, metric: str) -> int | float:
        if metric == "admin_mutation_audit_coverage_ratio":
            return 1.0
        if metric == "worker_dedicated_role_verified":
            return 1
        if metric in {"membership_acl_revoke_max_ms", "quiet_org_query_p95_ms"}:
            return 100
        return 0

    def _passing_report(self) -> dict:
        report = self._load_template()
        report["status"] = "pass"
        report["targetMatch"] = True
        report["markhandPhase1cGate"] = True
        report["embeddingProfile"] = "mock"
        report["metrics"] = {
            metric: self._metric_value(metric) for metric in GATES.PHASE1C_METRIC_THRESHOLDS
        }
        report["workerProof"] = {
            "runtimeRole": "markhand_worker",
            "dedicatedDatabaseUrlVerified": True,
            "superuser": False,
            "bypassRls": False,
            "verifiedAt": "2026-08-04T00:00:00Z",
        }
        report["redactionScan"] = {"passed": True}
        report["vulnerabilityScan"] = {
            "scanner": gate.pinned_trivy_image(),
            "undispositionedHighCritical": 0,
            "findings": [],
            "passed": True,
        }
        report["p1c8EvidenceMapping"] = [
            {"item": item, "gateId": gate_id, "evidence": row["evidence"]}
            for gate_id, items in gate.GATE_TO_P1C8.items()
            for item in items
            for row in GATES.G1C_GATE_ROWS
            if row["id"] == gate_id
        ]
        for result in report["gateResults"]:
            metric = result["metric"]
            result["value"] = report["metrics"][metric]
            result["pass"] = True
        report["canonicalBinding"] = {
            "registryRevision": 1,
            **GATES.phase1c_canonical_fingerprints(
                self.markhand_root, workspace_root=self.repo_root
            )[0],
        }
        report["git"] = {"commit": self.git_commit, "dirty": False}
        return report

    def _write_evidence(self, *, include_p1c8: bool = True) -> None:
        scenario_by_gate = {
            row["id"]: GATES.PHASE1C_WORKLOAD_SCENARIOS[index % len(GATES.PHASE1C_WORKLOAD_SCENARIOS)]
            for index, row in enumerate(GATES.G1C_GATE_ROWS)
        }
        for row in GATES.G1C_GATE_ROWS:
            gate_id = str(row["id"])
            path = self.repo_root / str(row["evidence"])
            path.parent.mkdir(parents=True, exist_ok=True)
            payload = evidence_payload(
                gate_id,
                scenario=scenario_by_gate[gate_id],
            )
            if not include_p1c8:
                payload.pop("p1c8Items", None)
            path.write_text(json.dumps(payload) + "\n", encoding="utf-8")

    def evaluate(self, report: dict, *, evidence_must_exist: bool = True) -> tuple[str, list[str]]:
        return gate.evaluate_report(
            report,
            repo_root=self.repo_root,
            markhand_root=self.markhand_root,
            evidence_must_exist=evidence_must_exist,
        )

    def test_default_template_is_not_run(self) -> None:
        status, blockers = self.evaluate(self._load_template(), evidence_must_exist=False)
        self.assertEqual(status, "not_run")
        self.assertTrue(blockers)

    def test_rejects_pass_with_target_match_false(self) -> None:
        report = self._passing_report()
        report["targetMatch"] = False
        status, blockers = self.evaluate(report)
        self.assertNotEqual(status, "pass")
        self.assertTrue(
            any("targetMatch" in item or "status pass requires targetMatch" in item for item in blockers)
        )

    def test_rejects_worker_runtime_role_markhand_app(self) -> None:
        report = self._passing_report()
        report["workerProof"]["runtimeRole"] = "markhand_app"
        self._write_evidence()
        status, blockers = self.evaluate(report)
        self.assertNotEqual(status, "pass")
        self.assertTrue(any("runtimeRole must be markhand_worker" in item for item in blockers))

    def test_rejects_superuser_worker(self) -> None:
        report = self._passing_report()
        report["workerProof"]["superuser"] = True
        self._write_evidence()
        status, blockers = self.evaluate(report)
        self.assertNotEqual(status, "pass")
        self.assertTrue(any("workerProof.superuser must be false" in item for item in blockers))

    def test_rejects_bypassrls_worker(self) -> None:
        report = self._passing_report()
        report["workerProof"]["bypassRls"] = True
        self._write_evidence()
        status, blockers = self.evaluate(report)
        self.assertNotEqual(status, "pass")
        self.assertTrue(any("workerProof.bypassRls must be false" in item for item in blockers))

    def test_rejects_cloud_embedding_profile(self) -> None:
        report = self._passing_report()
        report["embeddingProfile"] = "cloud-shared"
        self._write_evidence()
        status, blockers = self.evaluate(report)
        self.assertNotEqual(status, "pass")
        self.assertTrue(any("embedding_profile" in item for item in blockers))

    def test_rejects_leakage_above_zero(self) -> None:
        report = self._passing_report()
        report["metrics"]["cross_tenant_leakage_count"] = 1
        for result in report["gateResults"]:
            if result["gateId"] == "G1C-SEC-LEAKAGE":
                result["value"] = 1
                result["pass"] = False
        self._write_evidence()
        status, blockers = self.evaluate(report)
        self.assertNotEqual(status, "pass")
        self.assertTrue(
            any("cross_tenant_leakage_count" in item or "violates" in item for item in blockers)
        )

    def test_rejects_undispositioned_high_critical(self) -> None:
        report = self._passing_report()
        report["vulnerabilityScan"]["undispositionedHighCritical"] = 2
        report["metrics"]["undispositioned_high_critical_count"] = 2
        self._write_evidence()
        status, blockers = self.evaluate(report)
        self.assertNotEqual(status, "pass")
        self.assertTrue(any("undispositionedHighCritical" in item for item in blockers))

    def test_rejects_secret_leakage_in_report(self) -> None:
        report = self._passing_report()
        report["notes"] = "Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxIn0.sig"
        self._write_evidence()
        status, blockers = self.evaluate(report)
        self.assertNotEqual(status, "pass")
        self.assertTrue(any("redaction" in item or "secret" in item for item in blockers))

    def test_rejects_absolute_path_in_report(self) -> None:
        report = self._passing_report()
        report["notes"] = "/workspace/leak/path"
        self._write_evidence()
        status, blockers = self.evaluate(report)
        self.assertNotEqual(status, "pass")
        self.assertTrue(
            any(
                "absolute_path" in item
                or "redaction" in item
                or "/workspace/" in item
                for item in blockers
            )
        )

    def test_rejects_missing_p1c8_evidence_mapping(self) -> None:
        report = self._passing_report()
        report.pop("p1c8EvidenceMapping", None)
        self._write_evidence(include_p1c8=False)
        status, blockers = self.evaluate(report)
        self.assertNotEqual(status, "pass")
        self.assertTrue(any("p1c8" in item for item in blockers))

    def test_rejects_malformed_metric_types(self) -> None:
        report = self._passing_report()
        report["metrics"]["starvation_events"] = "zero"
        self._write_evidence()
        status, blockers = self.evaluate(report)
        self.assertNotEqual(status, "pass")
        self.assertTrue(blockers)

    def test_rejects_missing_evidence_files(self) -> None:
        report = self._passing_report()
        status, blockers = self.evaluate(report, evidence_must_exist=True)
        self.assertNotEqual(status, "pass")
        self.assertTrue(any("missing" in item or "evidence" in item for item in blockers))

    def test_rejects_duplicate_gate_results(self) -> None:
        report = self._passing_report()
        report["gateResults"].append(dict(report["gateResults"][0]))
        self._write_evidence()
        status, blockers = self.evaluate(report)
        self.assertNotEqual(status, "pass")
        self.assertTrue(any("duplicate" in item for item in blockers))

    def test_rejects_registry_metric_mismatch(self) -> None:
        report = self._passing_report()
        report["gateResults"][0]["metric"] = "unknown_metric"
        self._write_evidence()
        status, blockers = self.evaluate(report)
        self.assertNotEqual(status, "pass")
        self.assertTrue(any("metric must match registry" in item for item in blockers))

    def test_rejects_unpinned_trivy_scanner(self) -> None:
        report = self._passing_report()
        report["vulnerabilityScan"]["scanner"] = "aquasec/trivy:latest"
        self._write_evidence()
        status, blockers = self.evaluate(report)
        self.assertNotEqual(status, "pass")
        self.assertTrue(any("scanner_pin" in item or "trivy" in item for item in blockers))

    def test_rejects_canonical_binding_drift(self) -> None:
        report = self._passing_report()
        report["canonicalBinding"]["gatesSha256"] = "f" * 64
        self._write_evidence()
        status, blockers = self.evaluate(report)
        self.assertNotEqual(status, "pass")
        self.assertTrue(any("diverges from canonical" in item for item in blockers))

    def test_rejects_probe_command_failure(self) -> None:
        report = self._passing_report()
        self._write_evidence()
        path = self.repo_root / "bench/markhand_web/reports/phase-1c-gate/revoke.json"
        payload = json.loads(path.read_text(encoding="utf-8"))
        payload["probe"]["commandExitCode"] = 1
        path.write_text(json.dumps(payload) + "\n", encoding="utf-8")
        status, blockers = self.evaluate(report)
        self.assertNotEqual(status, "pass")
        self.assertTrue(any("probe_exit" in item or "commandExitCode" in item for item in blockers))

    def test_rejects_probe_timeout_or_partial_output(self) -> None:
        report = self._passing_report()
        self._write_evidence()
        path = self.repo_root / "bench/markhand_web/reports/phase-1c-gate/leakage.json"
        payload = json.loads(path.read_text(encoding="utf-8"))
        payload["probe"]["timedOut"] = True
        payload["probe"]["eof"] = False
        path.write_text(json.dumps(payload) + "\n", encoding="utf-8")
        status, blockers = self.evaluate(report)
        self.assertNotEqual(status, "pass")
        self.assertTrue(any("probe_timeout" in item or "probe_eof" in item for item in blockers))

    def test_atomic_write_purges_on_redaction_failure(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "phase-1c-gate.json"
            report = self._passing_report()
            self._write_evidence()
            report["notes"] = "postgres://user:secret@127.0.0.1/db"
            with self.assertRaises(gate.HarnessWriteError):
                gate.atomic_write_report(out, report, repo_root=self.repo_root)
            self.assertFalse(out.exists())

    def test_passing_fixture_validates_green(self) -> None:
        report = self._passing_report()
        self._write_evidence()
        status, blockers = self.evaluate(report)
        self.assertEqual(status, "pass", msg=f"unexpected blockers: {blockers}")
        self.assertEqual(blockers, [])


class Phase1cHarnessCiRoutingTests(unittest.TestCase):
    def test_rust_test_registered_as_ignored_e2e_phase1c_gate(self) -> None:
        listing = subprocess.check_output(
            [
                "cargo",
                "test",
                "-p",
                "fileconv-server",
                "--test",
                "e2e_phase1c_gate",
                "--",
                "--list",
            ],
            cwd=gate.ROOT,
            text=True,
            stderr=subprocess.STDOUT,
        )
        self.assertIn("e2e_phase1c_gate", listing)
        self.assertRegex(listing, r"e2e_phase1c_gate\s*:\s*test")

    def test_ci_skips_e2e_phase1c_gate_in_rust_integration(self) -> None:
        ci = (gate.ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        self.assertIn("--skip e2e_phase1c_gate", ci)
        self.assertIn("MARKHAND_PHASE1C_GATE=1", ci)


if __name__ == "__main__":
    unittest.main()
