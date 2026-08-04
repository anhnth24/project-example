#!/usr/bin/env python3
"""RED/GREEN tests for deployed Phase 1C probe architecture (HTTP/SQL/compose)."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[3]
PROBES_PATH = ROOT / "bench/markhand_web/scripts/phase1c_deployed_probes.py"
GATE_PATH = ROOT / "bench/markhand_web/scripts/run_phase1c_gate.py"


def load_probes():
    spec = importlib.util.spec_from_file_location("phase1c_deployed_probes", PROBES_PATH)
    if spec is None or spec.loader is None:
        raise ImportError("phase1c_deployed_probes.py missing")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def load_gate():
    spec = importlib.util.spec_from_file_location("run_phase1c_gate", GATE_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class DeployedArchitectureContractTests(unittest.TestCase):
    def test_deployed_probes_module_exists(self) -> None:
        self.assertTrue(PROBES_PATH.is_file(), "phase1c_deployed_probes.py required")

    def test_gate_has_no_cargo_probe_specs_for_pass(self) -> None:
        gate = load_gate()
        self.assertFalse(
            hasattr(gate, "CARGO_PROBE_SPECS") and gate.CARGO_PROBE_SPECS,
            "qualifying PASS must not use CARGO_PROBE_SPECS",
        )
        self.assertTrue(hasattr(gate, "DEPLOYED_PROBE_GATES"))

    def test_nearest_rank_p95_from_perf_counter_ns(self) -> None:
        probes = load_probes()
        samples = [100_000_000, 200_000_000, 300_000_000, 400_000_000, 500_000_000]
        p95_ms = probes.nearest_rank_p95_ms(samples)
        self.assertEqual(p95_ms, 500)

    def test_quota_drift_matches_production_formula(self) -> None:
        probes = load_probes()
        drift = probes.compute_quota_drift(
            documents=0,
            storage_bytes=0,
            reserved_concurrent_slots=0,
        )
        self.assertEqual(drift, 0)
        drift_bad = probes.compute_quota_drift(
            documents=3,
            storage_bytes=999,
            reserved_concurrent_slots=1,
        )
        self.assertEqual(drift_bad, 1003)

    def test_manifest_sha256_matches_canonical_bytes(self) -> None:
        probes = load_probes()
        manifest = ROOT / "crates/server/tests/fixtures/multi-org-denial.manifest.json"
        expected = hashlib.sha256(manifest.read_bytes()).hexdigest()
        self.assertEqual(probes.canonical_denial_manifest_sha256(), expected)

    def test_trivy_parser_requires_target_digest_match(self) -> None:
        probes = load_probes()
        digest = "sha256:" + "a" * 64
        report = {
            "SchemaVersion": 2,
            "ArtifactName": f"markhand-api:poc@{digest}",
            "Results": [{"Target": f"markhand-api:poc ({digest})", "Vulnerabilities": []}],
        }
        probes.validate_trivy_report_target(report, requested_ref=f"markhand-api:poc@{digest}")
        with self.assertRaises(RuntimeError):
            probes.validate_trivy_report_target(
                report,
                requested_ref="markhand-api:poc@sha256:" + "b" * 64,
            )

    def test_trivy_includes_unfixed_high_critical(self) -> None:
        probes = load_probes()
        report = {
            "SchemaVersion": 2,
            "ArtifactName": "markhand-api:poc@sha256:" + "a" * 64,
            "Results": [
                {
                    "Target": "markhand-api:poc",
                    "Vulnerabilities": [
                        {
                            "VulnerabilityID": "CVE-2026-9999",
                            "Severity": "CRITICAL",
                            "Status": "unknown",
                        }
                    ],
                }
            ],
        }
        findings = probes.extract_high_critical_findings(report)
        self.assertEqual(len(findings), 1)

    def test_seed_fixture_requires_challenge_binding(self) -> None:
        probes = load_probes()
        raw = {
            "schemaVersion": 1,
            "challenge": "phase1c-challenge-abc",
            "orgAlphaId": "11111111-1111-1111-1111-111111111111",
            "orgBetaId": "22222222-2222-2222-2222-222222222222",
            "markerAlpha": "phase1c-marker-alpha",
            "markerBeta": "phase1c-marker-beta",
        }
        fixture = probes.parse_seed_artifact(raw, expected_challenge="phase1c-challenge-abc")
        self.assertEqual(fixture.marker_alpha, "phase1c-marker-alpha")
        with self.assertRaises(RuntimeError):
            probes.parse_seed_artifact(raw, expected_challenge="wrong-challenge")

    def test_echo_shim_without_side_effects_fails_deployed_runner(self) -> None:
        probes = load_probes()
        calls: list[str] = []

        class EchoShims(probes.DeployedProbeShims):
            def http_request(self, **kwargs):  # type: ignore[override]
                calls.append("http")
                return probes.HttpResponse(status=200, body="{}", headers={})

            def compose(self, args, **kwargs):  # type: ignore[override]
                calls.append("compose")
                return probes.CommandOutcome(exit_code=0, stdout="", stderr="")

            def psql(self, sql, **kwargs):  # type: ignore[override]
                calls.append("psql")
                return probes.CommandOutcome(exit_code=0, stdout="[]", stderr="")

        seed = probes.SeedFixture(
            challenge="c1",
            org_alpha_id="11111111-1111-1111-1111-111111111111",
            org_beta_id="22222222-2222-2222-2222-222222222222",
            marker_alpha="phase1c-marker-alpha",
            marker_beta="phase1c-marker-beta",
            manifest_sha256=probes.canonical_denial_manifest_sha256(),
            source_revision={"commit": "c" * 40, "dirty": False},
        )
        runner = probes.DeployedProbeRunner(
            api_base="http://127.0.0.1:8788",
            seed=seed,
            shims=EchoShims(),
            noisy_duration_secs=1,
        )
        with self.assertRaises(RuntimeError):
            runner.run_http_denial_probe()
        self.assertLess(len(calls), 3, "echo shim must not satisfy probe without correlated transitions")

    def test_deployed_runner_tracks_http_compose_psql_transitions(self) -> None:
        probes = load_probes()
        journal: list[str] = []

        class TrackingShims(probes.DeployedProbeShims):
            def http_request(self, **kwargs):  # type: ignore[override]
                journal.append(f"http:{kwargs.get('method')}:{kwargs.get('path')}")
                if kwargs.get("path") == "/api/v1/orgs/switch":
                    return probes.HttpResponse(status=200, body='{"orgId":"11111111-1111-1111-1111-111111111111"}', headers={})
                if "foreign" in str(kwargs.get("path", "")):
                    return probes.HttpResponse(status=404, body="{}", headers={})
                return probes.HttpResponse(status=403, body="{}", headers={})

            def compose(self, args, **kwargs):  # type: ignore[override]
                journal.append("compose:" + " ".join(args[:2]))
                return probes.CommandOutcome(exit_code=0, stdout="", stderr="")

            def psql(self, sql, **kwargs):  # type: ignore[override]
                journal.append("psql:" + sql[:40])
                return probes.CommandOutcome(exit_code=0, stdout="0", stderr="")

        seed = probes.SeedFixture(
            challenge="c1",
            org_alpha_id="11111111-1111-1111-1111-111111111111",
            org_beta_id="22222222-2222-2222-2222-222222222222",
            marker_alpha="phase1c-marker-alpha",
            marker_beta="phase1c-marker-beta",
            alpha_owner_token="tok-alpha",
            beta_owner_token="tok-beta",
            alpha_foreign_collection_id="aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
            manifest_sha256=probes.canonical_denial_manifest_sha256(),
            source_revision={"commit": "c" * 40, "dirty": False},
        )
        runner = probes.DeployedProbeRunner(
            api_base="http://127.0.0.1:8788",
            seed=seed,
            shims=TrackingShims(),
            noisy_duration_secs=1,
        )
        outcome = runner.run_http_denial_probe()
        self.assertEqual(outcome.metrics["cross_tenant_leakage_count"], 0)
        self.assertTrue(any(item.startswith("http:") for item in journal))

    def test_worker_nonce_must_match_harness_challenge(self) -> None:
        gate = load_gate()
        nonce = "harness-nonce-007"
        line = json.dumps(
            {
                "schemaVersion": 1,
                "currentUser": "markhand_worker",
                "superuser": False,
                "bypassRls": False,
                "dedicatedDatabaseUrlVerified": True,
                "databaseUrlRolePath": "markhand_worker",
                "nonce": nonce,
            }
        )
        stdout = f"PHASE1C_WORKER_ROLE_PROBE\t{line}\nPHASE1C_WORKER_ROLE_PROBE_EOF\ttrue\n"
        parsed = gate.parse_worker_role_probe(stdout, expected_nonce=nonce)
        self.assertEqual(parsed["nonce"], nonce)
        with self.assertRaises(RuntimeError):
            gate.parse_worker_role_probe(stdout, expected_nonce="other-nonce")

    def test_staging_commits_report_last(self) -> None:
        gate = load_gate()
        with tempfile.TemporaryDirectory() as tmp:
            final_dir = Path(tmp) / "out"
            final_dir.mkdir()
            staging = gate.StagingWorkspace(
                repo_root=Path(tmp),
                final_dir=final_dir,
                source_revision={"commit": "c" * 40, "dirty": False},
            )
            staging.stage_json("bench/markhand_web/reports/phase-1c-gate/leakage.json", {"gateId": "G1C-SEC-LEAKAGE", "status": "pass"})
            staging.stage_json("bench/markhand_web/reports/phase-1c-gate/phase-1c-gate.json", {"status": "pass"})
            staging.commit_all_report_last()
            self.assertTrue((final_dir / "phase-1c-gate.json").is_file())
            mtime_report = (final_dir / "phase-1c-gate.json").stat().st_mtime
            mtime_evidence = (final_dir / "leakage.json").stat().st_mtime
            self.assertGreaterEqual(mtime_report, mtime_evidence)

    def test_output_dir_python_bypass_rejected(self) -> None:
        gate = load_gate()
        evil = Path(tempfile.gettempdir()) / "phase1c-evil"
        evil.mkdir(exist_ok=True)
        symlink = evil / "link.json"
        if symlink.exists() or symlink.is_symlink():
            symlink.unlink()
        outside = evil / "outside.json"
        outside.write_text("{}\n", encoding="utf-8")
        symlink.symlink_to(outside)
        staging = gate.StagingWorkspace(
            repo_root=gate.ROOT,
            final_dir=evil,
            source_revision={"commit": "c" * 40, "dirty": False},
        )
        gates_spec = importlib.util.spec_from_file_location(
            "check_markhand_gates", ROOT / "scripts/check-markhand-gates.py"
        )
        assert gates_spec and gates_spec.loader
        gates_mod = importlib.util.module_from_spec(gates_spec)
        gates_spec.loader.exec_module(gates_mod)
        with self.assertRaises((RuntimeError, gate.HarnessWriteError, OSError)):
            staging.commit_file(
                symlink,
                relative_path=sorted(gates_mod.PHASE1C_EVIDENCE_ALLOWLIST)[0],
            )


class DeployedCiRouteTests(unittest.TestCase):
    def test_ci_failure_uploads_safe_diagnostic_not_raw_logs(self) -> None:
        ci = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        job = ci.split("phase1c-g1c-security-gate:", 1)[1].split("deployed-1c-integration:", 1)[0]
        self.assertIn("Upload Phase 1C gate failure report", job)
        self.assertIn("phase1c-gate-failure.json", job)
        dump_idx = job.index("Dump POC logs on failure")
        enforce_idx = job.index("Enforce G1C gate")
        upload_fail_idx = job.index("Upload Phase 1C gate failure report")
        self.assertLess(enforce_idx, upload_fail_idx)
        self.assertLess(dump_idx, enforce_idx)


if __name__ == "__main__":
    unittest.main()
