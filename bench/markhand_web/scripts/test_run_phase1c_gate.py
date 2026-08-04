#!/usr/bin/env python3
"""Hermetic contract tests for the Phase 1C deployed harness."""

from __future__ import annotations

import importlib.util
import inspect
import json
import os
import shutil
import subprocess
import sys
import tempfile
import textwrap
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


def evidence_payload(gate_id: str, *, scenario: str, repo_root: Path, markhand_root: Path, extra: dict | None = None) -> dict:
    git_commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo_root, text=True).strip()
    probe = {
        "commandExitCode": 0,
        "timedOut": False,
        "outputTruncated": False,
        "eof": True,
        "durationMs": 1,
    }
    metrics = {
        "cross_tenant_leakage_count": 0,
        "post_commit_stale_authorizations": 0,
        "membership_acl_revoke_max_ms": 100,
        "quota_drift_after_recovery": 0,
        "quiet_org_query_p95_ms": 100,
        "starvation_events": 0,
        "admin_mutation_audit_coverage_ratio": 1.0,
        "worker_dedicated_role_verified": 1,
        "undispositioned_high_critical_count": 0,
    }
    row = next(r for r in GATES.G1C_GATE_ROWS if r["id"] == gate_id)
    gate_metrics = {
        metric: metrics[metric]
        for metric in row["metrics"]  # type: ignore[index]
    }
    payload = gate.build_evidence_payload(
        gate_id=gate_id,
        scenario=scenario,
        probe=probe,
        metrics=gate_metrics,
        source_revision={"commit": git_commit, "dirty": False},
        markhand_root=markhand_root,
        repo_root=repo_root,
        metrics_observed=True,
    )
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
                repo_root=self.repo_root,
                markhand_root=self.markhand_root,
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
        self.assertNotEqual(status, "pass")

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

    def test_rejects_pass_when_evidence_coverage_limited(self) -> None:
        report = self._passing_report()
        self._write_evidence()
        path = self.repo_root / "bench/markhand_web/reports/phase-1c-gate/leakage.json"
        payload = json.loads(path.read_text(encoding="utf-8"))
        payload["coverageLimited"] = True
        payload["coverageLimitedReasons"] = ["denial:denial-resolveCitation-citation:citation_repeat"]
        payload["status"] = "fail"
        path.write_text(json.dumps(payload) + "\n", encoding="utf-8")
        status, blockers = self.evaluate(report)
        self.assertNotEqual(status, "pass")
        self.assertTrue(any("coverage_limited" in item for item in blockers))

    def test_rejects_pass_when_metrics_not_observed(self) -> None:
        report = self._passing_report()
        self._write_evidence()
        path = self.repo_root / "bench/markhand_web/reports/phase-1c-gate/noisy-neighbor.json"
        payload = json.loads(path.read_text(encoding="utf-8"))
        payload["metricsObserved"] = False
        payload["status"] = "fail"
        path.write_text(json.dumps(payload) + "\n", encoding="utf-8")
        status, blockers = self.evaluate(report)
        self.assertNotEqual(status, "pass")
        self.assertTrue(any("metrics_not_observed" in item for item in blockers))

    def test_rejects_pass_when_report_declares_coverage_limited(self) -> None:
        report = self._passing_report()
        report["coverageLimited"] = ["denial:approveIntake"]
        self._write_evidence()
        status, blockers = self.evaluate(report)
        self.assertNotEqual(status, "pass")
        self.assertTrue(any("report_coverage_limited" in item for item in blockers))

    def test_build_evidence_payload_marks_coverage_limited_non_pass(self) -> None:
        payload = gate.build_evidence_payload(
            gate_id="G1C-SEC-LEAKAGE",
            scenario="cross_tenant_denial",
            probe={"commandExitCode": 0, "timedOut": False, "outputTruncated": False, "eof": True},
            metrics={"cross_tenant_leakage_count": 0},
            source_revision={"commit": self.git_commit, "dirty": False},
            markhand_root=self.markhand_root,
            repo_root=self.repo_root,
            coverage_limited=True,
            coverage_limited_reasons=["denial:citation_repeat"],
            metrics_observed=False,
        )
        self.assertTrue(payload["coverageLimited"])
        self.assertNotEqual(payload["status"], "pass")
        self.assertFalse(payload["metricsObserved"])

    def test_deployed_probe_to_command_probe_propagates_coverage_limited(self) -> None:
        probes_mod = gate._DEPLOYED
        result = probes_mod.DeployedProbeResult(
            gate_id="G1C-SEC-QUOTA-RECOVERY",
            probe={"deployedApi": True, "eof": True},
            metrics={},
            coverage_limited=True,
            coverage_limited_reasons=["quota:docker_unavailable"],
            metrics_observed=False,
        )
        command_probe = probes_mod.deployed_probe_to_command_probe(result)
        self.assertTrue(command_probe.get("coverageLimited"))
        self.assertIn("quota:docker_unavailable", command_probe.get("coverageLimitedReasons", []))
        self.assertFalse(command_probe.get("metricsObserved", True))


class Phase1cBackboneEndToEndTests(unittest.TestCase):
    """End-to-end no-false-PASS backbone: runner + check-markhand-gates + assembly."""

    def setUp(self) -> None:
        self._temp = tempfile.TemporaryDirectory()
        self.repo_root = Path(self._temp.name)
        self.markhand_root, self.repo_root = GATES.prepare_phase1c_fixture(self.repo_root)
        self.git_commit = init_git_repo(self.repo_root)

    def tearDown(self) -> None:
        self._temp.cleanup()

    def _write_qualifying_evidence(self, *, coverage_on_leakage: bool = False) -> None:
        scenario_by_gate = {
            row["id"]: GATES.PHASE1C_WORKLOAD_SCENARIOS[index % len(GATES.PHASE1C_WORKLOAD_SCENARIOS)]
            for index, row in enumerate(GATES.G1C_GATE_ROWS)
        }
        for row in GATES.G1C_GATE_ROWS:
            gate_id = str(row["id"])
            payload = gate.build_evidence_payload(
                gate_id=gate_id,
                scenario=scenario_by_gate[gate_id],
                probe={"commandExitCode": 0, "timedOut": False, "outputTruncated": False, "eof": True},
                metrics={
                    metric: (1.0 if metric == "admin_mutation_audit_coverage_ratio" else 100 if "ms" in metric else 0 if metric != "worker_dedicated_role_verified" else 1)
                    for metric in row["metrics"]  # type: ignore[index]
                },
                source_revision={"commit": self.git_commit, "dirty": False},
                markhand_root=self.markhand_root,
                repo_root=self.repo_root,
                metrics_observed=True,
                coverage_limited=coverage_on_leakage and gate_id == "G1C-SEC-LEAKAGE",
                coverage_limited_reasons=["denial:test"] if coverage_on_leakage and gate_id == "G1C-SEC-LEAKAGE" else None,
            )
            path = self.repo_root / str(row["evidence"])
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(json.dumps(payload) + "\n", encoding="utf-8")

    def _passing_report(self) -> dict:
        report = GATES.load_json_yaml(
            self.markhand_root / "reports/phase-1c-gate/phase-1c-gate.template.json"
        )
        report["status"] = "pass"
        report["targetMatch"] = True
        report["metrics"] = {
            metric: (1.0 if metric.endswith("_ratio") else 1 if metric == "worker_dedicated_role_verified" else 100 if "ms" in metric else 0)
            for metric in GATES.PHASE1C_METRIC_THRESHOLDS
        }
        report["workerProof"] = {
            "runtimeRole": "markhand_worker",
            "dedicatedDatabaseUrlVerified": True,
            "superuser": False,
            "bypassRls": False,
            "verifiedAt": "2026-08-04T00:00:00Z",
        }
        report["redactionScan"] = {"passed": True}
        report["vulnerabilityScan"]["passed"] = True
        report["vulnerabilityScan"]["undispositionedHighCritical"] = 0
        report["git"] = {"commit": self.git_commit, "dirty": False}
        report["canonicalBinding"] = {
            "registryRevision": 1,
            **GATES.phase1c_canonical_fingerprints(self.markhand_root, workspace_root=self.repo_root)[0],
        }
        return report

    def test_rejects_pass_when_metrics_observed_missing(self) -> None:
        report = self._passing_report()
        self._write_qualifying_evidence()
        path = self.repo_root / "bench/markhand_web/reports/phase-1c-gate/leakage.json"
        payload = json.loads(path.read_text(encoding="utf-8"))
        payload.pop("metricsObserved", None)
        path.write_text(json.dumps(payload) + "\n", encoding="utf-8")
        status, blockers = gate.evaluate_report(
            report,
            repo_root=self.repo_root,
            markhand_root=self.markhand_root,
        )
        self.assertNotEqual(status, "pass")
        self.assertTrue(any("metrics_not_observed" in item for item in blockers))

    def test_build_evidence_default_is_not_observed(self) -> None:
        payload = gate.build_evidence_payload(
            gate_id="G1C-SEC-LEAKAGE",
            scenario="cross_tenant_denial",
            probe={"commandExitCode": 0, "timedOut": False, "outputTruncated": False, "eof": True},
            metrics={"cross_tenant_leakage_count": 0},
            source_revision={"commit": self.git_commit, "dirty": False},
            markhand_root=self.markhand_root,
            repo_root=self.repo_root,
        )
        self.assertFalse(payload.get("metricsObserved"))

    def test_assemble_report_rejects_coverage_limited_evidence(self) -> None:
        self._write_qualifying_evidence(coverage_on_leakage=True)
        metrics = {metric: (1.0 if metric.endswith("_ratio") else 1 if metric == "worker_dedicated_role_verified" else 100 if "ms" in metric else 0) for metric in GATES.PHASE1C_METRIC_THRESHOLDS}
        with self.assertRaises(RuntimeError):
            gate.assemble_pass_report(
                metrics,
                {"runtimeRole": "markhand_worker", "dedicatedDatabaseUrlVerified": True, "superuser": False, "bypassRls": False, "verifiedAt": "2026-08-04T00:00:00Z"},
                {"scanner": gate.pinned_trivy_image(), "undispositionedHighCritical": 0, "findings": [], "passed": True},
                repo_root=self.repo_root,
                markhand_root=self.markhand_root,
                source_revision={"commit": self.git_commit, "dirty": False},
            )

    def test_fully_observed_evidence_assembles_schema_valid_pass(self) -> None:
        self._write_qualifying_evidence()
        metrics = {metric: (1.0 if metric.endswith("_ratio") else 1 if metric == "worker_dedicated_role_verified" else 100 if "ms" in metric else 0) for metric in GATES.PHASE1C_METRIC_THRESHOLDS}
        report = gate.assemble_pass_report(
            metrics,
            {"runtimeRole": "markhand_worker", "dedicatedDatabaseUrlVerified": True, "superuser": False, "bypassRls": False, "verifiedAt": "2026-08-04T00:00:00Z"},
            {"scanner": gate.pinned_trivy_image(), "undispositionedHighCritical": 0, "findings": [], "passed": True},
            repo_root=self.repo_root,
            markhand_root=self.markhand_root,
            source_revision={"commit": self.git_commit, "dirty": False},
        )
        self.assertEqual(report["status"], "pass")
        self.assertEqual(report.get("coverageLimited"), [])
        for result in report["gateResults"]:
            self.assertTrue(result.get("metricsObserved"))
            self.assertFalse(result.get("coverageLimited"))
            self.assertTrue(result["pass"])
        schema = GATES.load_json_yaml(self.markhand_root / "schema/phase1c-gate-report.schema.json")
        schema_errors = GATES.schema_errors(report, schema, "assembled-report")
        self.assertEqual(schema_errors, [], msg=f"schema errors: {schema_errors}")

    def test_check_markhand_gates_rejects_pass_with_coverage_limited_evidence(self) -> None:
        self._write_qualifying_evidence(coverage_on_leakage=True)
        report = self._passing_report()
        errors = GATES.phase1c_gate_report_errors(
            report,
            registry=GATES.load_json_yaml(self.markhand_root / "gates.yaml"),
            root=self.markhand_root,
            repo_root=self.repo_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("coverageLimited" in err for err in errors))

    def test_check_markhand_gates_rejects_pass_without_metrics_observed(self) -> None:
        self._write_qualifying_evidence()
        path = self.repo_root / "bench/markhand_web/reports/phase-1c-gate/noisy-neighbor.json"
        payload = json.loads(path.read_text(encoding="utf-8"))
        payload.pop("metricsObserved", None)
        path.write_text(json.dumps(payload) + "\n", encoding="utf-8")
        report = self._passing_report()
        errors = GATES.phase1c_gate_report_errors(
            report,
            registry=GATES.load_json_yaml(self.markhand_root / "gates.yaml"),
            root=self.markhand_root,
            repo_root=self.repo_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("metricsObserved" in err for err in errors))


class Phase1cReviewFixContractTests(unittest.TestCase):
    """Review-fix RED tests (Commit C): must fail until harness implementation lands."""

    def test_run_live_probes_accepts_injectable_command_runner(self) -> None:
        params = inspect.signature(gate.run_live_probes).parameters
        self.assertIn(
            "command_runner",
            params,
            "run_live_probes must accept injectable command_runner for behavioral tests",
        )

    def test_staging_workspace_class_exists(self) -> None:
        self.assertTrue(
            hasattr(gate, "StagingWorkspace"),
            "harness must stage evidence in a private temp workspace",
        )

    def test_parse_denial_report_requires_leakage_count(self) -> None:
        self.assertTrue(hasattr(gate, "parse_denial_report"))
        with self.assertRaises((RuntimeError, ValueError, KeyError, TypeError)):
            gate.parse_denial_report({"schemaVersion": 1, "summary": {}})

    def test_parse_denial_report_binds_leakage_count_not_summary_field(self) -> None:
        metrics = gate.parse_denial_report(
            {
                "schemaVersion": 1,
                "leakageCount": 0,
                "failures": [],
                "manifestSha256": "a" * 64,
                "gitShaFull": "b" * 40,
                "redactionScan": {"passed": True, "findings": []},
            }
        )
        self.assertEqual(metrics["cross_tenant_leakage_count"], 0)

    def test_parse_probe_result_requires_eof_and_rejects_trailing_output(self) -> None:
        self.assertTrue(hasattr(gate, "parse_probe_stdout"))
        payload = json.dumps(
            {"schemaVersion": 1, "probeId": "revoke", "metrics": {"membership_acl_revoke_max_ms": 12}}
        )
        stdout = f"PHASE1C_PROBE_RESULT\t{payload}\nPHASE1C_PROBE_EOF\ttrue\nextra\n"
        with self.assertRaises(RuntimeError):
            gate.parse_probe_stdout(stdout, probe_id="revoke")

    def test_parse_probe_result_rejects_missing_or_duplicate_markers(self) -> None:
        payload = json.dumps(
            {"schemaVersion": 1, "probeId": "revoke", "metrics": {"membership_acl_revoke_max_ms": 12}}
        )
        with self.assertRaises(RuntimeError):
            gate.parse_probe_stdout("noise\n", probe_id="revoke")
        dup = f"PHASE1C_PROBE_RESULT\t{payload}\nPHASE1C_PROBE_RESULT\t{payload}\nPHASE1C_PROBE_EOF\ttrue\n"
        with self.assertRaises(RuntimeError):
            gate.parse_probe_stdout(dup, probe_id="revoke")

    def test_parse_worker_role_probe_strict_json_contract(self) -> None:
        nonce = "phase1c-worker-nonce-001"
        line = json.dumps(
            {
                "schemaVersion": 1,
                "currentUser": "markhand_worker",
                "superuser": False,
                "bypassRls": False,
                "dedicatedDatabaseUrlVerified": True,
                "databaseUrlRolePath": "markhand_worker",
                "nonce": nonce,
            },
            sort_keys=True,
        )
        stdout = f"PHASE1C_WORKER_ROLE_PROBE\t{line}\nPHASE1C_WORKER_ROLE_PROBE_EOF\ttrue\n"
        parsed = gate.parse_worker_role_probe(stdout)
        self.assertEqual(parsed["currentUser"], "markhand_worker")
        self.assertIs(parsed["superuser"], False)
        self.assertIs(parsed["bypassRls"], False)
        self.assertEqual(parsed["nonce"], nonce)

    def test_parse_worker_role_probe_rejects_preamble_lines(self) -> None:
        line = json.dumps(
            {
                "schemaVersion": 1,
                "currentUser": "markhand_worker",
                "superuser": False,
                "bypassRls": False,
                "dedicatedDatabaseUrlVerified": True,
                "databaseUrlRolePath": "markhand_worker",
                "nonce": "n1",
            },
            sort_keys=True,
        )
        stdout = f"noise before probe\nPHASE1C_WORKER_ROLE_PROBE\t{line}\nPHASE1C_WORKER_ROLE_PROBE_EOF\ttrue\n"
        with self.assertRaises(RuntimeError):
            gate.parse_worker_role_probe(stdout)

    def test_parse_worker_role_probe_rejects_string_booleans(self) -> None:
        line = json.dumps(
            {
                "schemaVersion": 1,
                "currentUser": "markhand_worker",
                "superuser": "false",
                "bypassRls": "false",
                "dedicatedDatabaseUrlVerified": True,
                "databaseUrlRolePath": "markhand_worker",
                "nonce": "n1",
            }
        )
        stdout = f"PHASE1C_WORKER_ROLE_PROBE\t{line}\nPHASE1C_WORKER_ROLE_PROBE_EOF\ttrue\n"
        with self.assertRaises(RuntimeError):
            gate.parse_worker_role_probe(stdout)

    def test_parse_worker_role_probe_rejects_multiple_payloads(self) -> None:
        line = json.dumps(
            {
                "schemaVersion": 1,
                "currentUser": "markhand_worker",
                "superuser": False,
                "bypassRls": False,
                "dedicatedDatabaseUrlVerified": True,
                "databaseUrlRolePath": "markhand_worker",
                "nonce": "n1",
            },
            sort_keys=True,
        )
        stdout = (
            f"PHASE1C_WORKER_ROLE_PROBE\t{line}\n"
            f"PHASE1C_WORKER_ROLE_PROBE\t{line}\n"
            f"PHASE1C_WORKER_ROLE_PROBE_EOF\ttrue\n"
        )
        with self.assertRaises(RuntimeError):
            gate.parse_worker_role_probe(stdout)

    def test_parse_trivy_reports_combines_both_images(self) -> None:
        self.assertTrue(hasattr(gate, "parse_combined_trivy_scan"))
        digest_a = "sha256:" + "a" * 64
        digest_b = "sha256:" + "b" * 64
        api_report = {
            "SchemaVersion": 2,
            "ArtifactName": digest_a,
            "Results": [
                {
                    "Target": f"markhand-api:poc ({digest_a})",
                    "Vulnerabilities": [
                        {"VulnerabilityID": "CVE-2026-0001", "Severity": "HIGH"}
                    ],
                }
            ],
        }
        worker_report = {
            "SchemaVersion": 2,
            "ArtifactName": digest_b,
            "Results": [{"Target": f"markhand-worker:poc ({digest_b})", "Vulnerabilities": []}],
        }
        outcome = gate.parse_combined_trivy_scan(
            api_report=api_report,
            worker_report=worker_report,
            api_ref=digest_a,
            worker_ref=digest_b,
            trivyignore_text="# empty\n",
        )
        self.assertEqual(outcome["undispositionedHighCritical"], 1)
        self.assertEqual(len(outcome["images"]), 2)

    def test_parse_trivy_rejects_malformed_partial_reports(self) -> None:
        with self.assertRaises(RuntimeError):
            gate.parse_combined_trivy_scan(
                api_report={"Results": "bad"},
                worker_report={"SchemaVersion": 2, "Results": []},
                api_ref="markhand-api:poc@sha256:" + "a" * 64,
                worker_ref="markhand-worker:poc@sha256:" + "b" * 64,
                trivyignore_text="",
            )

    def test_run_live_probes_fails_on_noop_command_runner(self) -> None:
        def noop_runner(command, **kwargs):
            return {
                "command": command,
                "commandExitCode": 0,
                "timedOut": False,
                "outputTruncated": False,
                "eof": True,
                "durationMs": 1,
                "stdout": "",
                "stderr": "",
                "stdoutSha256": "0" * 64,
                "stderrSha256": "0" * 64,
                "residualSecrets": False,
            }

        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            markhand, repo = GATES.prepare_phase1c_fixture(repo)
            init_git_repo(repo)
            env = {
                **os.environ,
                "MARKHAND_TEST_REQUIRED": "1",
                "COMPOSE_PROFILES": "mock",
                "MARKHAND_PHASE1C_GATE": "1",
            }
            with mock.patch.dict(os.environ, env, clear=False):
                with self.assertRaises((RuntimeError, OSError, FileNotFoundError)):
                    gate.run_live_probes(
                        repo,
                        markhand,
                        command_runner=noop_runner,
                    )

    def test_evidence_payload_includes_binding_fields(self) -> None:
        self.assertTrue(hasattr(gate, "build_evidence_payload"))
        probe = {
            "commandExitCode": 0,
            "timedOut": False,
            "outputTruncated": False,
            "eof": True,
            "durationMs": 1,
        }
        payload = gate.build_evidence_payload(
            gate_id="G1C-SEC-LEAKAGE",
            scenario="multi_org_denial_replay",
            probe=probe,
            metrics={"cross_tenant_leakage_count": 0},
            source_revision={"commit": "c" * 40, "dirty": False},
            markhand_root=gate.MARKHAND_ROOT,
            repo_root=gate.ROOT,
        )
        for field in (
            "schemaVersion",
            "evidencePath",
            "environmentId",
            "workloadProfileId",
            "embeddingProfile",
            "sourceRevision",
            "canonicalBinding",
            "thresholdDecisions",
            "targetMatch",
        ):
            self.assertIn(field, payload, f"missing evidence binding field {field}")

    def test_failure_message_redaction_helper_exists(self) -> None:
        self.assertTrue(hasattr(gate, "sanitize_failure_message"))
        raw = "probe failed: postgres://user:secret@127.0.0.1/db /workspace/leak"
        cleaned = gate.sanitize_failure_message(raw)
        self.assertNotIn("secret", cleaned)
        self.assertNotIn("/workspace/", cleaned)

    def test_staging_purges_allowlisted_final_artifacts_on_failure(self) -> None:
        self.assertTrue(hasattr(gate, "StagingWorkspace"))
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            markhand, repo = GATES.prepare_phase1c_fixture(repo)
            git_commit = init_git_repo(repo)
            final_dir = repo / "out"
            final_dir.mkdir()
            decoy = final_dir / "leakage.json"
            decoy.write_text('{"status":"pass"}\n', encoding="utf-8")
            staging = gate.StagingWorkspace(
                repo_root=repo,
                final_dir=final_dir,
                source_revision={"commit": git_commit, "dirty": False},
            )
            staging.purge_final_allowlisted_artifacts()
            self.assertFalse(decoy.exists())

    def test_staging_rejects_symlink_escape(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            markhand, repo = GATES.prepare_phase1c_fixture(repo)
            git_commit = init_git_repo(repo)
            final_dir = repo / "out"
            final_dir.mkdir()
            outside = repo / "outside-secret.json"
            outside.write_text('{"leak":true}\n', encoding="utf-8")
            staging_root = repo / ".staging"
            staging_root.mkdir()
            evil = staging_root / "evil.json"
            evil.symlink_to(outside)
            staging = gate.StagingWorkspace(
                repo_root=repo,
                final_dir=final_dir,
                source_revision={"commit": git_commit, "dirty": False},
            )
            with self.assertRaises((RuntimeError, gate.HarnessWriteError, OSError)):
                staging.commit_file(evil, relative_path=sorted(GATES.PHASE1C_EVIDENCE_ALLOWLIST)[0])


class Phase1cShellEntrypointTests(unittest.TestCase):
    SHELL = gate.ROOT / "deploy/scripts/g1c-security-gate.sh"

    def test_output_dir_flag_before_validate_args(self) -> None:
        proc = subprocess.run(
            [
                "bash",
                str(self.SHELL),
                "--output-dir",
                "/tmp/phase1c-shell-test",
                "--validate-args",
            ],
            cwd=gate.ROOT,
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertEqual(
            proc.returncode,
            0,
            msg=textwrap.shorten(proc.stderr or proc.stdout, 500),
        )
        self.assertIn("phase1c-gate-args-ok", proc.stdout)

    def test_rejects_unknown_option(self) -> None:
        proc = subprocess.run(
            ["bash", str(self.SHELL), "--unknown-flag"],
            cwd=gate.ROOT,
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertNotEqual(proc.returncode, 0)

    def test_rejects_duplicate_output_dir(self) -> None:
        proc = subprocess.run(
            [
                "bash",
                str(self.SHELL),
                "--output-dir",
                "/tmp/a",
                "--output-dir",
                "/tmp/b",
            ],
            cwd=gate.ROOT,
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertNotEqual(proc.returncode, 0)

    def test_rejects_unsafe_output_dir_traversal(self) -> None:
        proc = subprocess.run(
            ["bash", str(self.SHELL), "--output-dir", "../../etc/passwd"],
            cwd=gate.ROOT,
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertNotEqual(proc.returncode, 0)


class Phase1cSeedScriptContractTests(unittest.TestCase):
    SEED = gate.ROOT / "deploy/scripts/phase1c-multi-org-seed.sh"
    SEED_PY = gate.ROOT / "bench/markhand_web/scripts/phase1c_multi_org_seed.py"

    def test_seed_script_delegates_to_python_module(self) -> None:
        text = self.SEED.read_text(encoding="utf-8")
        self.assertIn("phase1c_multi_org_seed.py", text)
        self.assertNotIn("accessToken", text)

    def test_seed_python_module_has_schema_validation(self) -> None:
        text = self.SEED_PY.read_text(encoding="utf-8")
        self.assertIn("schemaVersion", text)
        self.assertIn("build_public_seed_evidence", text)

    def test_seed_shell_purges_credentials_on_exit(self) -> None:
        text = self.SEED.read_text(encoding="utf-8")
        self.assertIn("trap", text)
        self.assertIn("HUP", text)
        self.assertIn("purge_phase1c_credentials", text)
        self.assertNotIn("trap -", text)


class Phase1cDeployedArchitectureTests(unittest.TestCase):
    def test_deployed_probes_module_required(self) -> None:
        path = gate.ROOT / "bench/markhand_web/scripts/phase1c_deployed_probes.py"
        self.assertTrue(path.is_file(), "deployed probe module required for qualifying PASS")

    def test_no_cargo_probe_specs_for_qualifying_pass(self) -> None:
        self.assertFalse(
            getattr(gate, "CARGO_PROBE_SPECS", None),
            "CARGO_PROBE_SPECS must be removed; use deployed HTTP/SQL/compose probes",
        )
        self.assertTrue(hasattr(gate, "DEPLOYED_PROBE_GATES"))

    def test_run_live_probes_accepts_deployed_context(self) -> None:
        params = inspect.signature(gate.run_live_probes).parameters
        self.assertIn("deployed_context", params)


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

    def test_ci_enforces_gate_before_artifact_upload(self) -> None:
        ci = (gate.ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        upload_idx = ci.index("Upload Phase 1C gate evidence")
        enforce_idx = ci.index("Enforce G1C gate")
        self.assertLess(enforce_idx, upload_idx)

    def test_ci_upload_not_unconditional_on_always(self) -> None:
        ci = (gate.ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        job = ci.split("phase1c-g1c-security-gate:", 1)[1].split("deployed-1c-integration:", 1)[0]
        upload_block = job.split("Upload Phase 1C gate evidence", 1)[1].split("retention-days:", 1)[0]
        self.assertIn("if: success()", upload_block)
        self.assertNotIn("if: always()", upload_block)


if __name__ == "__main__":
    unittest.main()
