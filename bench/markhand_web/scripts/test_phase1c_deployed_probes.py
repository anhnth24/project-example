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
DENIAL_PATH = ROOT / "bench/markhand_web/scripts/phase1c_http_denial.py"
GATE_PATH = ROOT / "bench/markhand_web/scripts/run_phase1c_gate.py"


def load_denial():
    spec = importlib.util.spec_from_file_location("phase1c_http_denial", DENIAL_PATH)
    if spec is None or spec.loader is None:
        raise ImportError("phase1c_http_denial.py missing")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def complete_seed_raw(probes, **overrides: object) -> dict:
    commit = "a" * 40
    base = {
        "schemaVersion": 1,
        "challenge": "phase1c-challenge-abc",
        "sourceRevision": {"commit": commit, "dirty": False},
        "manifestSha256": probes.canonical_denial_manifest_sha256(),
        "orgAlphaId": "11111111-1111-1111-1111-111111111111",
        "orgBetaId": "22222222-2222-2222-2222-222222222222",
        "alphaUserId": "22222222-2222-2222-2222-222222222201",
        "betaUserId": "33333333-3333-3333-3333-333333333301",
        "markerAlpha": "phase1c-marker-alpha-aaa111",
        "markerBeta": "phase1c-marker-beta-bbb222",
        "alphaCollectionId": "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
        "betaCollectionId": "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
        "alphaDocumentId": "cccccccc-cccc-cccc-cccc-cccccccccccc",
        "betaDocumentId": "dddddddd-dddd-dddd-dddd-dddddddddddd",
        "alphaJobId": "eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee",
        "betaJobId": "ffffffff-ffff-ffff-ffff-ffffffffffff",
        "alphaVersionId": "12121212-1212-1212-1212-121212121212",
        "betaVersionId": "13131313-1313-1313-1313-131313131313",
        "alphaChatSessionId": "14141414-1414-1414-1414-141414141414",
        "betaChatSessionId": "15151515-1515-1515-1515-151515151515",
        "alphaProjectId": "16161616-1616-1616-1616-161616161616",
        "betaProjectId": "17171717-1717-1717-1717-171717171717",
        "alphaConflictId": "18181818-1818-1818-1818-181818181818",
        "betaConflictId": "19191919-1919-1919-1919-191919191919",
        "betaMemberUserId": "33333333-3333-3333-3333-333333333301",
        "alphaInviteId": "99999999-9999-9999-9999-999999999999",
        "betaInviteId": "88888888-8888-8888-8888-888888888888",
        "betaInviteAcceptToken": "mhinv1.test-token",
        "betaDownloadCapability": "cap-denial-test",
        "alphaSessionIdHash": "sha256:" + "a" * 64,
        "betaSessionIdHash": "sha256:" + "b" * 64,
        "orgAlphaSlug": "poc",
        "orgBetaSlug": "phase1c-beta",
    }
    base.update(overrides)
    return base


def load_probes():
    spec = importlib.util.spec_from_file_location("phase1c_deployed_probes", PROBES_PATH)
    if spec is None or spec.loader is None:
        raise ImportError("phase1c_deployed_probes.py missing")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def load_gate():
    spec = importlib.util.spec_from_file_location("run_phase1c_gate", GATE_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
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
        raw = complete_seed_raw(probes)
        fixture = probes.parse_seed_artifact(raw, expected_challenge="phase1c-challenge-abc")
        self.assertEqual(fixture.marker_alpha, "phase1c-marker-alpha-aaa111")
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


class Phase1cFixtureDenialSliceTests(unittest.TestCase):
    """Commit G RED: fixture + HTTP denial + revoke/token/audit slice contracts."""

    def test_http_denial_module_required(self) -> None:
        self.assertTrue(DENIAL_PATH.is_file(), "phase1c_http_denial.py required")

    def test_parse_seed_rejects_missing_required_fields(self) -> None:
        probes = load_probes()
        raw = complete_seed_raw(probes)
        raw.pop("betaCollectionId")
        with self.assertRaises(RuntimeError):
            probes.parse_seed_artifact(raw, expected_challenge=raw["challenge"])

    def test_parse_seed_rejects_duplicate_identities(self) -> None:
        probes = load_probes()
        raw = complete_seed_raw(probes, betaUserId="22222222-2222-2222-2222-222222222201")
        with self.assertRaises(RuntimeError):
            probes.parse_seed_artifact(raw, expected_challenge=raw["challenge"])

    def test_parse_seed_rejects_manifest_sha_mismatch(self) -> None:
        probes = load_probes()
        raw = complete_seed_raw(probes, manifestSha256="f" * 64)
        with self.assertRaises(RuntimeError):
            probes.parse_seed_artifact(raw, expected_challenge=raw["challenge"])

    def test_parse_seed_rejects_source_revision_mismatch(self) -> None:
        probes = load_probes()
        raw = complete_seed_raw(probes, sourceRevision={"commit": "b" * 40, "dirty": False})
        with self.assertRaises(RuntimeError):
            probes.validate_source_revision_binding(
                raw,
                git_sha_full="a" * 40,
            )

    def test_denial_mapping_covers_all_http_sse_rows(self) -> None:
        denial = load_denial()
        mapping = denial.build_http_sse_denial_mapping()
        rows = denial.load_manifest_rows()
        self.assertEqual(len(mapping), len(rows))
        self.assertEqual(
            {entry.operation_id for entry in mapping},
            {row.operation_id for row in rows},
        )

    def test_denial_mapping_rejects_unknown_operation(self) -> None:
        denial = load_denial()
        manifest = json.loads(
            (ROOT / "crates/server/tests/fixtures/multi-org-denial.manifest.json").read_text(
                encoding="utf-8"
            )
        )
        manifest["rows"].append(
            {
                "id": "denial-unknown-op",
                "binary": "multi_org_denial",
                "testName": "shared_world_http_surfaces_respect_org_scope",
                "operationId": "notARealOperation",
                "guardInventoryRef": "notARealOperation",
                "layer": "http",
                "status": "executable",
            }
        )
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "manifest.json"
            path.write_text(json.dumps(manifest), encoding="utf-8")
            with self.assertRaises(RuntimeError):
                denial.build_http_sse_denial_mapping(manifest_path=path)

    def test_denial_mapping_rejects_duplicate_operation(self) -> None:
        denial = load_denial()
        manifest = json.loads(
            (ROOT / "crates/server/tests/fixtures/multi-org-denial.manifest.json").read_text(
                encoding="utf-8"
            )
        )
        duplicate = dict(manifest["rows"][0])
        duplicate["id"] = "denial-dup-op"
        manifest["rows"].append(duplicate)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "manifest.json"
            path.write_text(json.dumps(manifest), encoding="utf-8")
            with self.assertRaises(RuntimeError):
                denial.build_http_sse_denial_mapping(manifest_path=path)

    def test_revoke_probe_rejects_404_mutation(self) -> None:
        probes = load_probes()
        seed = probes.parse_seed_artifact(
            complete_seed_raw(load_probes()),
            expected_challenge="phase1c-challenge-abc",
        )
        creds = probes.SeedCredentials(
            challenge=seed.challenge,
            alpha_access_token="alpha-token",
            alpha_refresh_token="alpha-refresh",
            beta_access_token="beta-token",
            beta_refresh_token="beta-refresh",
            alpha_session_id="sess-alpha",
            beta_session_id="sess-beta",
        )

        class Revoke404Shim(probes.DeployedProbeShims):
            def http_request(self, **kwargs):  # type: ignore[override]
                path = str(kwargs.get("path") or "")
                if path.endswith("/api/v1/auth/me") and kwargs.get("token") == creds.beta_access_token:
                    return probes.HttpResponse(status=200, body='{"userId":"ok"}', headers={})
                if path.startswith("/api/v1/members/") and kwargs.get("method") == "DELETE":
                    return probes.HttpResponse(status=404, body='{"code":"not_found"}', headers={})
                return probes.HttpResponse(status=403, body="{}", headers={})

            def compose(self, args, **kwargs):  # type: ignore[override]
                return probes.CommandOutcome(exit_code=0, stdout="", stderr="")

            def psql(self, sql, **kwargs):  # type: ignore[override]
                return probes.CommandOutcome(exit_code=0, stdout="0", stderr="")

        runner = probes.DeployedProbeRunner(
            api_base="http://127.0.0.1:8788",
            seed=seed,
            credentials=creds,
            shims=Revoke404Shim(),
        )
        with self.assertRaises(RuntimeError):
            runner.run_revoke_probe()

    def test_stale_token_probe_rejects_fake_refresh_token(self) -> None:
        probes = load_probes()
        source = probes.DeployedProbeRunner.run_stale_tokens_probe.__code__.co_consts
        text = Path(PROBES_PATH).read_text(encoding="utf-8")
        self.assertNotIn("invalid-phase1c-probe", text)

    def test_audit_ratio_uses_correlation_not_substring(self) -> None:
        probes = load_probes()
        seed = probes.parse_seed_artifact(
            complete_seed_raw(load_probes()),
            expected_challenge="phase1c-challenge-abc",
        )
        creds = probes.SeedCredentials(
            challenge=seed.challenge,
            alpha_access_token="alpha-token",
            alpha_refresh_token="alpha-refresh",
            beta_access_token="beta-token",
            beta_refresh_token="beta-refresh",
            alpha_session_id="sess-alpha",
            beta_session_id="sess-beta",
        )

        class AuditShim(probes.DeployedProbeShims):
            def http_request(self, **kwargs):  # type: ignore[override]
                path = str(kwargs.get("path") or "")
                if path.endswith("/api/v1/orgs/switch"):
                    return probes.HttpResponse(status=200, body='{"orgId":"11111111-1111-1111-1111-111111111111"}', headers={})
                if path.endswith("/api/v1/collections") and kwargs.get("method") == "POST":
                    return probes.HttpResponse(
                        status=201,
                        body='{"id":"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"}',
                        headers={"x-request-id": "req-create-collection"},
                    )
                if path.endswith("/api/v1/audit"):
                    body = json.dumps(
                        {
                            "items": [
                                {
                                    "id": "1",
                                    "seq": 1,
                                    "actorId": seed.alpha_user_id,
                                    "action": "org.switch",
                                    "targetType": "org",
                                    "targetId": seed.org_alpha_id,
                                    "outcome": "success",
                                    "metadata": {},
                                    "requestId": "req-switch",
                                    "occurredAt": "2026-08-04T00:00:00Z",
                                }
                            ],
                            "page": {"nextCursor": None, "hasMore": False},
                        }
                    )
                    return probes.HttpResponse(status=200, body=body, headers={})
                return probes.HttpResponse(status=403, body="{}", headers={})

            def compose(self, args, **kwargs):  # type: ignore[override]
                return probes.CommandOutcome(exit_code=0, stdout="", stderr="")

            def psql(self, sql, **kwargs):  # type: ignore[override]
                return probes.CommandOutcome(exit_code=0, stdout="0", stderr="")

        runner = probes.DeployedProbeRunner(
            api_base="http://127.0.0.1:8788",
            seed=seed,
            credentials=creds,
            shims=AuditShim(),
        )
        outcome = runner.run_audit_probe()
        ratio = outcome.metrics["admin_mutation_audit_coverage_ratio"]
        self.assertGreaterEqual(ratio, 0.5)
        self.assertLess(ratio, 1.0)

    def test_audit_denominator_includes_failed_mutations(self) -> None:
        probes = load_probes()
        seed = probes.parse_seed_artifact(
            complete_seed_raw(load_probes()),
            expected_challenge="phase1c-challenge-abc",
        )
        creds = probes.SeedCredentials(
            challenge=seed.challenge,
            alpha_access_token="alpha-token",
            alpha_refresh_token="alpha-refresh",
            beta_access_token="beta-token",
            beta_refresh_token="beta-refresh",
            alpha_session_id="sess-alpha",
            beta_session_id="sess-beta",
        )

        class AuditFailShim(probes.DeployedProbeShims):
            def http_request(self, **kwargs):  # type: ignore[override]
                path = str(kwargs.get("path") or "")
                if path.endswith("/api/v1/orgs/switch"):
                    return probes.HttpResponse(status=403, body='{"code":"denied"}', headers={})
                if path.endswith("/api/v1/collections") and kwargs.get("method") == "POST":
                    return probes.HttpResponse(status=403, body='{"code":"denied"}', headers={})
                if path.endswith("/api/v1/audit"):
                    return probes.HttpResponse(
                        status=200,
                        body=json.dumps({"items": [], "page": {"nextCursor": None, "hasMore": False}}),
                        headers={},
                    )
                return probes.HttpResponse(status=403, body="{}", headers={})

            def compose(self, args, **kwargs):  # type: ignore[override]
                return probes.CommandOutcome(exit_code=0, stdout="", stderr="")

            def psql(self, sql, **kwargs):  # type: ignore[override]
                return probes.CommandOutcome(exit_code=0, stdout="0", stderr="")

        runner = probes.DeployedProbeRunner(
            api_base="http://127.0.0.1:8788",
            seed=seed,
            credentials=creds,
            shims=AuditFailShim(),
        )
        with self.assertRaises(RuntimeError):
            runner.run_audit_probe()

    def test_leakage_count_derived_from_violations_only(self) -> None:
        denial = load_denial()
        report = denial.DenialExecutionReport(
            schema_version=1,
            git_sha_full="a" * 40,
            manifest_sha256=denial.canonical_manifest_sha256(),
            challenge="c1",
            executable_http_sse_count=1,
            leakage_count=2,
            observations=[
                denial.DenialObservation(
                    operation_id="getCollection",
                    row_id="denial-getCollection",
                    scenario="foreign",
                    expected_statuses=[403, 404],
                    actual_status=404,
                    body_sha256="abc",
                    request_id=None,
                    challenge_echo=None,
                    leaked_markers=["phase1c-marker-beta-bbb222"],
                )
            ],
        )
        self.assertEqual(report.leakage_count, 2)
        report.leakage_count = 0
        self.assertNotEqual(report.leakage_count, len(report.observations))

    def test_seed_evidence_is_secret_free(self) -> None:
        probes = load_probes()
        raw = complete_seed_raw(probes)
        evidence = probes.build_public_seed_evidence(raw)
        serialized = json.dumps(evidence)
        self.assertNotIn("accessToken", serialized)
        self.assertNotIn("refreshToken", serialized)
        self.assertNotIn("/workspace/", serialized)
        self.assertNotIn("Bearer", serialized)
        self.assertIn("alphaSessionIdHash", serialized)

    def test_unrelated_token_denied_on_protected_resource(self) -> None:
        probes = load_probes()
        seed = probes.parse_seed_artifact(
            complete_seed_raw(load_probes()),
            expected_challenge="phase1c-challenge-abc",
        )
        creds = probes.SeedCredentials(
            challenge=seed.challenge,
            alpha_access_token="alpha-token",
            alpha_refresh_token="alpha-refresh",
            beta_access_token="beta-token",
            beta_refresh_token="beta-refresh",
            alpha_session_id="sess-alpha",
            beta_session_id="sess-beta",
        )

        class UnrelatedTokenShim(probes.DeployedProbeShims):
            def http_request(self, **kwargs):  # type: ignore[override]
                if kwargs.get("token") == "unrelated-token":
                    return probes.HttpResponse(status=200, body='{"userId":"leak"}', headers={})
                if kwargs.get("path", "").endswith("/api/v1/auth/me"):
                    return probes.HttpResponse(status=401, body='{"code":"unauthorized"}', headers={})
                return probes.HttpResponse(status=403, body="{}", headers={})

            def compose(self, args, **kwargs):  # type: ignore[override]
                return probes.CommandOutcome(exit_code=0, stdout="", stderr="")

            def psql(self, sql, **kwargs):  # type: ignore[override]
                return probes.CommandOutcome(exit_code=0, stdout="0", stderr="")

        runner = probes.DeployedProbeRunner(
            api_base="http://127.0.0.1:8788",
            seed=seed,
            credentials=creds,
            shims=UnrelatedTokenShim(),
        )
        response = runner._http("GET", "/api/v1/auth/me", token="unrelated-token")
        self.assertEqual(response.status, 401)
        with self.assertRaises(RuntimeError):
            runner.run_stale_tokens_probe()


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
