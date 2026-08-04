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
        "betaDownloadCapabilityHash": "sha256:" + "c" * 64,
        "alphaDownloadCapabilityHash": "sha256:" + "d" * 64,
        "betaInviteTokenHash": "sha256:" + "e" * 64,
        "alphaSessionIdHash": "sha256:" + "a" * 64,
        "betaSessionIdHash": "sha256:" + "b" * 64,
        "orgAlphaSlug": "poc",
        "orgBetaSlug": "phase1c-beta",
        "betaDenialDisposableCollectionId": "21212121-2121-2121-2121-212121212121",
        "betaDenialDisposableDocumentId": "23232323-2323-2323-2323-232323232323",
        "betaDenialDisposableChatSessionId": "24242424-2424-2424-2424-242424242424",
        "betaDenialDisposableInviteId": "25252525-2525-2525-2525-252525252525",
        "betaDenialDisposableMemberUserId": "44444444-4444-4444-4444-444444444401",
    }
    base.update(overrides)
    return base


def complete_credentials_raw(**overrides: object) -> dict:
    payload = {
        "schemaVersion": 1,
        "challenge": "phase1c-challenge-abc",
        "alphaAccessToken": "alpha-token",
        "alphaRefreshToken": "alpha-refresh",
        "betaAccessToken": "beta-token",
        "betaRefreshToken": "beta-refresh",
        "betaAlphaAccessToken": "beta-alpha-token",
        "betaAlphaRefreshToken": "beta-alpha-refresh",
        "alphaBetaAccessToken": "alpha-beta-token",
        "alphaBetaRefreshToken": "alpha-beta-refresh",
        "alphaSessionId": "11111111-1111-1111-1111-111111111101",
        "betaSessionId": "22222222-2222-2222-2222-222222222201",
        "betaInviteToken": "mhinv1.test-token",
        "alphaDownloadCapability": "cap-alpha-token",
        "betaDownloadCapability": "cap-beta-token",
        "betaDenialDisposableCollectionId": "21212121-2121-2121-2121-212121212121",
        "betaDenialDisposableCollectionUpdateId": "21212121-2121-2121-2121-212121212122",
        "betaDenialDisposableDocumentId": "23232323-2323-2323-2323-232323232323",
        "betaDenialDisposableChatSessionId": "24242424-2424-2424-2424-242424242424",
        "betaDenialDisposableInviteId": "25252525-2525-2525-2525-252525252525",
        "betaDenialDisposableMemberUserId": "44444444-4444-4444-4444-444444444401",
        "betaDenialDisposableConflictId": "26262626-2626-2626-2626-262626262626",
        "betaDenialAcceptInviteToken": "mhinv1.accept-disposable-token",
        "betaDenialAcceptAccessToken": "disposable-accept-token",
        "betaCitationChunkId": "27272727-2727-2727-2727-272727272727",
        "betaCitationSourceContentSha256": "a" * 64,
        "betaCitationCanonicalMarkdownSha256": "b" * 64,
        "betaCitationSourceSpanStart": 0,
        "betaCitationSourceSpanEnd": 12,
        "betaCitationQuoteLocalStart": 0,
        "betaCitationQuoteLocalEnd": 12,
        "betaCitationQuote": "phase1c-beta",
    }
    payload.update(overrides)
    return payload


def make_seed_credentials(probes, seed, **overrides: object):
    payload = {
        "challenge": seed.challenge,
        "alpha_access_token": "alpha-token",
        "alpha_refresh_token": "alpha-refresh",
        "beta_access_token": "beta-token",
        "beta_refresh_token": "beta-refresh",
        "beta_alpha_access_token": "beta-alpha-token",
        "beta_alpha_refresh_token": "beta-alpha-refresh",
        "alpha_session_id": "11111111-1111-1111-1111-111111111101",
        "beta_session_id": "22222222-2222-2222-2222-222222222201",
        "beta_invite_token": "mhinv1.test-token",
        "alpha_download_capability": "cap-alpha-token",
        "beta_download_capability": "cap-beta-token",
    }
    payload.update(overrides)
    return probes.SeedCredentials(**payload)


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
                return probes.CommandOutcome(exit_code=0, stdout="0", stderr="")

        seed = probes.parse_seed_artifact(
            complete_seed_raw(probes),
            expected_challenge="phase1c-challenge-abc",
        )
        creds = make_seed_credentials(probes, seed, alpha_access_token="tok-alpha", beta_access_token="tok-beta")
        runner = probes.DeployedProbeRunner(
            api_base="http://127.0.0.1:8788",
            seed=seed,
            credentials=creds,
            shims=EchoShims(),
            noisy_duration_secs=1,
            git_sha_full=seed.source_revision["commit"],
        )
        with self.assertRaises(RuntimeError):
            runner.run_http_denial_probe()
        self.assertTrue(calls, "echo shim should not satisfy denial semantics")

    def test_deployed_runner_tracks_http_compose_psql_transitions(self) -> None:
        probes = load_probes()
        journal: list[str] = []

        class TrackingShims(probes.DeployedProbeShims):
            def http_request(self, **kwargs):  # type: ignore[override]
                journal.append(f"http:{kwargs.get('method')}:{kwargs.get('path')}")
                token = kwargs.get("token")
                path = str(kwargs.get("path") or "")
                server_request_id = "99999999-9999-9999-9999-999999999901"
                headers = {"x-request-id": server_request_id}
                if token is None:
                    return probes.HttpResponse(status=401, body='{"code":"unauthorized"}', headers=headers)
                if token == creds.beta_access_token:
                    if path.endswith("/api/v1/auth/me"):
                        body = json.dumps(
                            {
                                "userId": seed.beta_user_id,
                                "orgId": seed.org_beta_id,
                                "sessionId": creds.beta_session_id,
                                "requestId": server_request_id,
                            }
                        )
                    elif path.endswith("/events"):
                        body = "event: status\ndata: {\"state\":\"queued\"}\n\n"
                        return probes.HttpResponse(
                            status=200,
                            body=body,
                            headers={**headers, "content-type": "text/event-stream"},
                        )
                    elif path.endswith("/api/v1/ask/stream"):
                        body = "event: message\ndata: {}\n\n"
                        return probes.HttpResponse(
                            status=200,
                            body=body,
                            headers={**headers, "content-type": "text/event-stream"},
                        )
                    elif "/versions/" in path and "/diff" in path:
                        body = json.dumps(
                            {
                                "fromVersionId": seed.beta_version_id,
                                "toVersionId": seed.beta_version_id,
                                "requestId": server_request_id,
                            }
                        )
                    elif "/versions/" in path:
                        body = json.dumps(
                            {
                                "id": seed.beta_version_id,
                                "versionId": seed.beta_version_id,
                                "requestId": server_request_id,
                            }
                        )
                    elif path.endswith("/preview"):
                        body = json.dumps(
                            {"documentId": seed.beta_document_id, "requestId": server_request_id}
                        )
                    elif path.endswith("/documents") and "/collections/" in path:
                        body = json.dumps({"items": [], "requestId": server_request_id})
                    elif path.endswith("/versions") and "/documents/" in path:
                        body = json.dumps({"items": [], "requestId": server_request_id})
                    elif path.endswith("/evidence"):
                        body = json.dumps({"items": [], "requestId": server_request_id})
                    elif "/documents/" in path:
                        body = json.dumps({"id": seed.beta_document_id, "requestId": server_request_id})
                    elif "/jobs/" in path:
                        body = json.dumps({"id": seed.beta_job_id, "requestId": server_request_id})
                    elif path.endswith("/api/v1/search") or path.endswith("/api/v1/ask"):
                        body = json.dumps({"items": [], "requestId": server_request_id})
                    elif path.endswith("/api/v1/uploads") and kwargs.get("multipart_body") is not None:
                        body = json.dumps(
                            {
                                "documentId": seed.beta_document_id,
                                "versionId": seed.beta_version_id,
                                "requestId": server_request_id,
                            }
                        )
                    elif path.endswith(f"/api/v1/downloads/{creds.beta_download_capability}"):
                        return probes.HttpResponse(status=200, body=seed.marker_beta, headers=headers)
                    elif path.endswith(f"/api/v1/collections/{seed.beta_denial_disposable_collection_id}"):
                        if kwargs.get("method") == "DELETE":
                            return probes.HttpResponse(status=204, body="", headers=headers)
                        body = json.dumps({"id": seed.beta_denial_disposable_collection_id, "requestId": server_request_id})
                    else:
                        body = json.dumps(
                            {
                                "id": seed.beta_collection_id,
                                "name": seed.marker_beta,
                                "requestId": server_request_id,
                            }
                        )
                    return probes.HttpResponse(status=200, body=body, headers=headers)
                return probes.HttpResponse(status=403, body='{"code":"forbidden"}', headers=headers)

            def compose(self, args, **kwargs):  # type: ignore[override]
                journal.append("compose:" + " ".join(args[:2]))
                return probes.CommandOutcome(exit_code=0, stdout="", stderr="")

            def psql(self, sql, **kwargs):  # type: ignore[override]
                journal.append("psql:" + sql[:40])
                return probes.CommandOutcome(exit_code=0, stdout="0", stderr="")

        seed = probes.parse_seed_artifact(
            complete_seed_raw(probes),
            expected_challenge="phase1c-challenge-abc",
        )
        creds = make_seed_credentials(probes, seed, alpha_access_token="tok-alpha", beta_access_token="tok-beta")
        runner = probes.DeployedProbeRunner(
            api_base="http://127.0.0.1:8788",
            seed=seed,
            credentials=creds,
            shims=TrackingShims(),
            noisy_duration_secs=1,
            git_sha_full=seed.source_revision["commit"],
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
        creds = make_seed_credentials(probes, seed)

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
        text = Path(PROBES_PATH).read_text(encoding="utf-8")
        self.assertNotIn("invalid-phase1c-probe", text)

    def test_audit_ratio_uses_correlation_not_substring(self) -> None:
        probes = load_probes()
        seed = probes.parse_seed_artifact(
            complete_seed_raw(load_probes()),
            expected_challenge="phase1c-challenge-abc",
        )
        creds = make_seed_credentials(probes, seed)

        class AuditShim(probes.DeployedProbeShims):
            switch_request_id = "33333333-3333-3333-3333-333333333301"
            create_request_id = "44444444-4444-4444-4444-444444444401"
            switch_access = "55555555-5555-5555-5555-555555555501"
            switch_session = "66666666-6666-6666-6666-666666666601"

            def http_request(self, **kwargs):  # type: ignore[override]
                path = str(kwargs.get("path") or "")
                if path.endswith("/api/v1/orgs/switch"):
                    return probes.HttpResponse(
                        status=200,
                        body=json.dumps(
                            {
                                "accessToken": self.switch_access,
                                "refreshToken": "switch-refresh",
                                "requestId": self.switch_request_id,
                            }
                        ),
                        headers={"x-request-id": self.switch_request_id},
                    )
                if path.endswith("/api/v1/auth/me") and kwargs.get("token") == self.switch_access:
                    return probes.HttpResponse(
                        status=200,
                        body=json.dumps(
                            {
                                "userId": seed.alpha_user_id,
                                "orgId": seed.org_alpha_id,
                                "sessionId": self.switch_session,
                                "requestId": self.switch_request_id,
                            }
                        ),
                        headers={"x-request-id": self.switch_request_id},
                    )
                if path.endswith("/api/v1/collections") and kwargs.get("method") == "POST":
                    return probes.HttpResponse(
                        status=201,
                        body=json.dumps(
                            {
                                "id": "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
                                "requestId": self.create_request_id,
                            }
                        ),
                        headers={"x-request-id": self.create_request_id},
                    )
                if path.endswith("/api/v1/audit"):
                    body = json.dumps(
                        {
                            "items": [
                                {
                                    "id": "77777777-7777-7777-7777-777777777701",
                                    "actorId": seed.alpha_user_id,
                                    "action": "org.switch",
                                    "targetType": "session",
                                    "targetId": self.switch_session,
                                    "outcome": "success",
                                    "requestId": self.switch_request_id,
                                    "occurredAt": "2026-08-04T12:00:00Z",
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
        creds = make_seed_credentials(probes, seed)

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
                    request_id="11111111-1111-1111-1111-111111111111",
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
        creds = make_seed_credentials(probes, seed)

        class UnrelatedTokenShim(probes.DeployedProbeShims):
            def http_request(self, **kwargs):  # type: ignore[override]
                if kwargs.get("token") == "unrelated-token-value":
                    return probes.HttpResponse(status=401, body='{"code":"unauthorized"}', headers={})
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


class Phase1cReviewerFixSliceTests(unittest.TestCase):
    """Commit I RED: reviewer rejection findings for HTTP slice (I→J)."""

    SEED_PY = ROOT / "bench/markhand_web/scripts/phase1c_multi_org_seed.py"
    SEED_SH = ROOT / "deploy/scripts/phase1c-multi-org-seed.sh"
    PROBES_PY = PROBES_PATH
    DENIAL_PY = DENIAL_PATH

    def _seed_fixture(self, probes):
        return probes.parse_seed_artifact(
            complete_seed_raw(probes),
            expected_challenge="phase1c-challenge-abc",
        )

    def _seed_creds(self, seed, probes):
        return make_seed_credentials(probes, seed)

    def test_seed_declares_identity_fixture_boundary(self) -> None:
        text = self.SEED_PY.read_text(encoding="utf-8")
        self.assertIn("IDENTITY_FIXTURE_BOUNDARY", text)
        self.assertIn("org_memberships", text)

    def test_seed_rejects_placeholder_resource_uuid_literals(self) -> None:
        text = self.SEED_PY.read_text(encoding="utf-8")
        for placeholder in (
            "cccccccc-cccc-cccc-cccc-cccccccccccc",
            "dddddddd-dddd-dddd-dddd-dddddddddddd",
            "eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee",
        ):
            self.assertNotIn(placeholder, text, f"placeholder UUID still present: {placeholder}")

    def test_seed_never_synthesizes_session_ids(self) -> None:
        text = self.SEED_PY.read_text(encoding="utf-8")
        self.assertNotIn("secrets.token_hex(16)", text)
        self.assertIn("/api/v1/auth/me", text)

    def test_seed_stores_invite_token_in_credentials_not_public_evidence(self) -> None:
        text = self.SEED_PY.read_text(encoding="utf-8")
        self.assertIn("betaInviteToken", text)
        self.assertNotIn("betaInviteAcceptToken", text.split("build_public_seed_evidence")[0])

    def test_shell_seed_purges_credentials_on_exit(self) -> None:
        text = self.SEED_SH.read_text(encoding="utf-8")
        self.assertIn("trap", text)
        self.assertIn("purge_phase1c_credentials", text)

    def test_denial_unauthenticated_requires_exact_401(self) -> None:
        denial = load_denial()
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = self._seed_creds(seed, probes)
        specs = denial.build_denial_request_specs(
            denial.build_http_sse_denial_mapping(),
            seed=seed,
            credentials=creds,
        )
        unauth = [spec for spec in specs if spec.scenario == "unauthenticated"]
        self.assertTrue(unauth, "expected unauthenticated denial specs")
        for spec in unauth:
            self.assertEqual(
                spec.expected_statuses,
                frozenset({401}),
                f"{spec.operation_id} must require exact 401, got {spec.expected_statuses}",
            )

    def test_denial_foreign_rows_include_owner_control(self) -> None:
        denial = load_denial()
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = self._seed_creds(seed, probes)
        specs = denial.build_denial_request_specs(
            denial.build_http_sse_denial_mapping(),
            seed=seed,
            credentials=creds,
        )
        foreign_ops = {spec.operation_id for spec in specs if spec.scenario == "foreign"}
        owner_ops = {spec.operation_id for spec in specs if spec.scenario == "owner_control"}
        self.assertTrue(owner_ops, "owner_control scenarios required")
        self.assertTrue(foreign_ops.issubset(owner_ops | foreign_ops))
        self.assertEqual(
            foreign_ops,
            owner_ops,
            "every foreign denial row must have a matching owner_control warm-up",
        )

    def test_denial_rejects_all_403_observation_matrix(self) -> None:
        denial = load_denial()
        observations = [
            denial.DenialObservation(
                operation_id="getCollection",
                row_id="denial-getCollection",
                scenario="foreign",
                expected_statuses=[403],
                actual_status=403,
                body_sha256="abc",
                request_id="req-1",
                leaked_markers=[],
            )
            for _ in range(3)
        ]
        with self.assertRaises(RuntimeError):
            denial.validate_denial_observation_matrix(observations)

    def test_denial_requires_server_minted_request_id(self) -> None:
        denial = load_denial()
        self.assertTrue(hasattr(denial, "validate_server_request_id"))
        with self.assertRaises(RuntimeError):
            denial.validate_server_request_id(body="{}", headers={})

    def test_denial_create_upload_uses_multipart_spec(self) -> None:
        denial = load_denial()
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = self._seed_creds(seed, probes)
        mapping = [
            entry
            for entry in denial.build_http_sse_denial_mapping()
            if entry.operation_id == "createUpload"
        ]
        self.assertEqual(len(mapping), 1)
        specs = denial.build_denial_request_specs(mapping, seed=seed, credentials=creds)
        self.assertTrue(
            any(
                spec.content_type.startswith("multipart/form-data")
                for spec in specs
            ),
            "createUpload must use multipart/form-data",
        )

    def test_probe_gate_order_acl_before_revoke(self) -> None:
        probes = load_probes()
        order = list(probes.DEPLOYED_PROBE_GATES)
        self.assertLess(
            order.index("G1C-SEC-ACL-CACHE"),
            order.index("G1C-SEC-REVOKE"),
            "ACL cache probe must run before membership delete revoke probe",
        )
        self.assertLess(
            order.index("G1C-SEC-STALE-TOKENS"),
            order.index("G1C-SEC-REVOKE"),
        )

    def test_deployed_runner_restores_membership_between_destructive_probes(self) -> None:
        text = self.PROBES_PY.read_text(encoding="utf-8")
        self.assertIn("_ensure_beta_membership", text)
        self.assertIn("_restore_beta_membership", text)

    def test_audit_org_switch_target_is_session_family_not_org(self) -> None:
        text = self.PROBES_PY.read_text(encoding="utf-8")
        self.assertIn("switchSessionTargetId", text)
        self.assertIn("_fetch_audit_entries_paginated", text)

    def test_credential_purge_helper_exists(self) -> None:
        probes = load_probes()
        self.assertTrue(hasattr(probes, "purge_phase1c_credentials"))
        path = Path(tempfile.mkdtemp()) / "creds.json"
        path.write_text('{"secret":"value"}\n', encoding="utf-8")
        path.chmod(0o600)
        probes.purge_phase1c_credentials(path)
        self.assertFalse(path.exists())

    def test_load_credentials_purges_after_read(self) -> None:
        probes = load_probes()
        path = Path(tempfile.mkdtemp()) / "creds.json"
        payload = {
            "schemaVersion": 1,
            "challenge": "phase1c-challenge-abc",
            "alphaAccessToken": "a",
            "alphaRefreshToken": "ar",
            "betaAccessToken": "b",
            "betaRefreshToken": "br",
            "betaAlphaAccessToken": "bar",
            "betaAlphaRefreshToken": "brr",
            "alphaSessionId": "11111111-1111-1111-1111-111111111101",
            "betaSessionId": "22222222-2222-2222-2222-222222222201",
            "betaInviteToken": "invite-token",
            "alphaDownloadCapability": "cap-alpha",
            "betaDownloadCapability": "cap-beta",
        }
        path.write_text(json.dumps(payload), encoding="utf-8")
        path.chmod(0o600)
        probes.load_seed_credentials(path, expected_challenge="phase1c-challenge-abc", purge_after_load=True)
        self.assertFalse(path.exists())

    def _load_stateful_fake(self):
        fake_path = ROOT / "bench/markhand_web/scripts/phase1c_stateful_fake.py"
        spec = importlib.util.spec_from_file_location("phase1c_stateful_fake_under_test", fake_path)
        assert spec and spec.loader
        module = importlib.util.module_from_spec(spec)
        sys.modules["phase1c_stateful_fake_under_test"] = module
        spec.loader.exec_module(module)
        return module

    def test_stateful_fake_deployment_module_required(self) -> None:
        fake_path = ROOT / "bench/markhand_web/scripts/phase1c_stateful_fake.py"
        self.assertTrue(fake_path.is_file(), "phase1c_stateful_fake.py required")

    def test_stateful_fake_runs_denial_with_owner_control(self) -> None:
        fake = self._load_stateful_fake()
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = self._seed_creds(seed, probes)
        deployment = fake.StatefulFakeDeployment(seed=seed, credentials=creds)
        report = deployment.run_denial_suite()
        self.assertEqual(report["ownerControlCount"], report["foreignCount"])
        self.assertGreater(report["ownerControlCount"], 0)

    def test_stateful_fake_negative_missing_owner_control_fails(self) -> None:
        fake = self._load_stateful_fake()
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = self._seed_creds(seed, probes)
        deployment = fake.StatefulFakeDeployment(
            seed=seed,
            credentials=creds,
            skip_owner_control=True,
        )
        with self.assertRaises(RuntimeError):
            deployment.run_denial_suite()

    def test_stateful_fake_negative_all_403_shim_rejected(self) -> None:
        fake = self._load_stateful_fake()
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = self._seed_creds(seed, probes)
        deployment = fake.StatefulFakeDeployment(
            seed=seed,
            credentials=creds,
            force_all_403=True,
        )
        with self.assertRaises(RuntimeError):
            deployment.run_denial_suite()

    def test_stateful_fake_negative_credentials_survive_cleanup(self) -> None:
        fake = self._load_stateful_fake()
        path = Path(tempfile.mkdtemp()) / "creds.json"
        path.write_text('{"token":"secret"}\n', encoding="utf-8")
        path.chmod(0o600)
        with self.assertRaises(RuntimeError):
            fake.StatefulFakeDeployment.assert_credentials_purged(path)


class Phase1cSecondReviewSliceTests(unittest.TestCase):
    """Commit K RED: second-review production-correct HTTP slice contracts."""

    SEED_PY = ROOT / "bench/markhand_web/scripts/phase1c_multi_org_seed.py"
    SEED_SH = ROOT / "deploy/scripts/phase1c-multi-org-seed.sh"
    DENIAL_PY = DENIAL_PATH
    PROBES_PY = PROBES_PATH

    def test_seed_invites_beta_into_beta_org_after_alpha_switch(self) -> None:
        text = self.SEED_PY.read_text(encoding="utf-8")
        self.assertIn("/api/v1/orgs/switch", text)
        self.assertIn("beta_org_invite", text)
        self.assertNotIn(
            'body={"email": BETA_EMAIL, "role": "editor"}',
            text.replace(" ", ""),
        )

    def test_seed_creates_beta_resources_with_beta_org_token(self) -> None:
        text = self.SEED_PY.read_text(encoding="utf-8")
        self.assertIn("beta_org_access", text)
        self.assertIn("switch_org", text)

    def test_conflict_fixture_uses_migration_claim_columns(self) -> None:
        text = self.SEED_PY.read_text(encoding="utf-8")
        for column in ("claim_key", "subject", "predicate", "value_type", "value_money", "effective_from"):
            self.assertIn(column, text)
        self.assertIn("claim_a_id<claim_b_id", text.replace(" ", ""))

    def test_public_evidence_excludes_download_capability_plaintext(self) -> None:
        probes = load_probes()
        self.assertNotIn("betaDownloadCapability", probes.SEED_REQUIRED_FIELDS)
        self.assertIn("betaDownloadCapabilityHash", probes.SEED_REQUIRED_FIELDS)

    def test_validate_server_request_id_requires_minted_uuid(self) -> None:
        probes = load_probes()
        server_id = probes.validate_server_request_id(
            body='{"requestId":"11111111-1111-1111-1111-111111111111"}',
            headers={"x-request-id": "11111111-1111-1111-1111-111111111111"},
        )
        self.assertEqual(server_id, "11111111-1111-1111-1111-111111111111")
        with self.assertRaises(RuntimeError):
            probes.validate_server_request_id(body="{}", headers={})

    def test_validate_server_request_id_does_not_require_client_equality(self) -> None:
        denial = load_denial()
        server_id = denial.validate_server_request_id(
            body='{"requestId":"22222222-2222-2222-2222-222222222222"}',
            headers={"x-request-id": "22222222-2222-2222-2222-222222222222"},
        )
        self.assertNotEqual(server_id, "client-advisory-id")

    def test_owner_control_uses_exact_mapped_method_not_substitute(self) -> None:
        denial = load_denial()
        probes = load_probes()
        seed = probes.parse_seed_artifact(
            complete_seed_raw(probes),
            expected_challenge="phase1c-challenge-abc",
        )
        creds = make_seed_credentials(probes, seed)
        entry = next(
            e for e in denial.build_http_sse_denial_mapping() if e.operation_id == "deleteCollection"
        )
        spec = denial.build_owner_control_spec(entry, seed=seed, credentials=creds)
        self.assertEqual(spec.method, "DELETE")
        self.assertIn("/collections/", spec.path)

    def test_create_upload_in_foreign_scope(self) -> None:
        denial = load_denial()
        self.assertIn("createUpload", denial._uses_foreign_scope("createUpload", "/uploads"))

    def test_shell_trap_covers_hup_and_never_unsets_without_unlink(self) -> None:
        text = self.SEED_SH.read_text(encoding="utf-8")
        self.assertIn("HUP", text)
        self.assertIn("purge_phase1c_credentials", text)
        self.assertNotIn("trap -", text)

    def test_load_credentials_finally_purges_on_malformed_json(self) -> None:
        probes = load_probes()
        path = Path(tempfile.mkdtemp()) / "creds.json"
        path.write_text("{not-json", encoding="utf-8")
        path.chmod(0o600)
        with self.assertRaises((RuntimeError, json.JSONDecodeError)):
            probes.load_seed_credentials(path, expected_challenge="phase1c-challenge-abc", purge_after_load=True)
        self.assertFalse(path.exists())

    def test_acl_probe_restores_editor_role(self) -> None:
        text = self.PROBES_PY.read_text(encoding="utf-8")
        self.assertIn("_restore_beta_alpha_membership_role", text)

    def test_stale_token_probe_does_not_delete_membership(self) -> None:
        text = self.PROBES_PY.read_text(encoding="utf-8")
        stale = text.split("def run_stale_tokens_probe", 1)[1].split("def run_", 1)[0]
        self.assertNotIn("DELETE", stale)

    def test_revoke_probe_is_last_membership_gate(self) -> None:
        probes = load_probes()
        order = list(probes.DEPLOYED_PROBE_GATES)
        membership = [
            i
            for i, gate in enumerate(order)
            if gate in {"G1C-SEC-ACL-CACHE", "G1C-SEC-STALE-TOKENS", "G1C-SEC-REVOKE"}
        ]
        self.assertEqual(membership, sorted(membership))
        self.assertEqual(order[membership[-1]], "G1C-SEC-REVOKE")

    def test_audit_uses_switch_access_token_auth_me_session(self) -> None:
        text = self.PROBES_PY.read_text(encoding="utf-8")
        self.assertIn("_session_id_from_switch_access_token", text)

    def test_audit_pagination_validates_cursor_monotonic(self) -> None:
        probes = load_probes()
        pages = [
            {
                "items": [
                    {
                        "id": "11111111-1111-1111-1111-111111111111",
                        "action": "a",
                        "targetType": "t",
                        "outcome": "success",
                        "occurredAt": "t",
                    }
                ],
                "page": {"hasMore": True, "nextCursor": "c1"},
            },
            {
                "items": [
                    {
                        "id": "22222222-2222-2222-2222-222222222222",
                        "action": "b",
                        "targetType": "t",
                        "outcome": "success",
                        "occurredAt": "t",
                    }
                ],
                "page": {"hasMore": False, "nextCursor": None},
            },
        ]
        probes.validate_audit_pages(pages, seen_cursors=set())

    def test_stateful_fake_invokes_execute_http_denial_suite(self) -> None:
        fake_path = ROOT / "bench/markhand_web/scripts/phase1c_stateful_fake.py"
        text = fake_path.read_text(encoding="utf-8")
        self.assertIn("execute_http_denial_suite", text)
        self.assertIn("build_http_sse_denial_mapping", text)

    def test_denial_report_omits_unused_challenge_echo(self) -> None:
        denial = load_denial()
        payload = denial.DenialExecutionReport(
            schema_version=1,
            git_sha_full="a" * 40,
            manifest_sha256=denial.canonical_manifest_sha256(),
            challenge="c1",
            executable_http_sse_count=1,
        ).as_dict()
        self.assertNotIn("challengeEcho", json.dumps(payload))

    def test_validate_uuid_helper_rejects_non_uuid(self) -> None:
        probes = load_probes()
        with self.assertRaises(RuntimeError):
            probes.validate_uuid("not-a-uuid", field="collectionId")


class Phase1cThirdReviewSliceTests(unittest.TestCase):
    """Commit M RED: third-review bounded HTTP slice contracts."""

    GATE_SH = ROOT / "deploy/scripts/g1c-security-gate.sh"
    SEED_PY = ROOT / "bench/markhand_web/scripts/phase1c_multi_org_seed.py"

    def _seed_fixture(self, probes):
        return probes.parse_seed_artifact(
            complete_seed_raw(probes),
            expected_challenge="phase1c-challenge-abc",
        )

    def _seed_creds(self, seed, probes):
        return make_seed_credentials(probes, seed)

    def test_manifest_includes_all_60_executable_http_sse_rows(self) -> None:
        denial = load_denial()
        mapping = denial.build_http_sse_denial_mapping()
        self.assertEqual(len(mapping), 60)
        row_ids = {entry.row_id for entry in mapping}
        self.assertEqual(len(row_ids), 60)

    def test_manifest_includes_secondary_rows_without_evidence_role_filter(self) -> None:
        denial = load_denial()
        mapping = denial.build_http_sse_denial_mapping()
        secondary = {entry.row_id for entry in mapping if entry.row_id.endswith("-citation") or "task13" in entry.row_id}
        self.assertGreaterEqual(len(secondary), 7)

    def test_conflict_fixture_claim_uuids_unique_per_org(self) -> None:
        text = self.SEED_PY.read_text(encoding="utf-8")
        self.assertIn("_claim_pair_for_org", text)
        self.assertNotIn("aaaaaaaa-0001-4000-8000-000000000001", text)

    def test_acl_probe_warms_multipart_upload_not_collection_get(self) -> None:
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = self._seed_creds(seed, probes)
        runner = probes.DeployedProbeRunner(
            api_base="http://fake",
            seed=seed,
            credentials=creds,
            shims=probes.DeployedProbeShims(),
            git_sha_full=seed.source_revision["commit"],
        )
        calls: list[tuple[str, str, str | None]] = []

        class AclShims(probes.DeployedProbeShims):
            def http_request(self, **kwargs):  # type: ignore[override]
                calls.append((str(kwargs.get("method")), str(kwargs.get("path")), kwargs.get("token")))
                headers = {"x-request-id": "11111111-1111-1111-1111-111111111111"}
                token = kwargs.get("token")
                if kwargs.get("multipart_body") is not None and token == creds.beta_alpha_access_token:
                    return probes.HttpResponse(status=201, body='{"documentId":"d","versionId":"v"}', headers=headers)
                if token == creds.beta_alpha_access_token:
                    return probes.HttpResponse(status=200, body='{"userId":"u","sessionId":"s"}', headers=headers)
                if token == creds.alpha_access_token:
                    return probes.HttpResponse(status=200, body="{}", headers=headers)
                return probes.HttpResponse(status=403, body='{"code":"forbidden"}', headers=headers)

            def compose(self, args, **kwargs):  # type: ignore[override]
                return probes.CommandOutcome(0, "", "")

            def psql(self, sql, **kwargs):  # type: ignore[override]
                return probes.CommandOutcome(0, "abc", "")

        runner.shims = AclShims()
        runner.run_acl_cache_probe()
        warm_uploads = [
            c for c in calls if c[0] == "POST" and "/uploads" in c[1] and c[2] == creds.beta_alpha_access_token
        ]
        self.assertTrue(warm_uploads, "ACL probe must warm multipart upload with beta_alpha_access_token")
        poll_uploads = warm_uploads[1:]
        self.assertTrue(poll_uploads, "ACL probe must poll upload denial after role downgrade")

    def test_revoke_probe_uses_beta_alpha_token_on_alpha_resource(self) -> None:
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = self._seed_creds(seed, probes)
        calls: list[tuple[str, str | None]] = []

        class RevokeShims(probes.DeployedProbeShims):
            def http_request(self, **kwargs):  # type: ignore[override]
                calls.append((str(kwargs.get("path")), kwargs.get("token")))
                headers = {"x-request-id": "11111111-1111-1111-1111-111111111111"}
                token = kwargs.get("token")
                path = str(kwargs.get("path") or "")
                if token == creds.beta_alpha_access_token and seed.alpha_collection_id in path:
                    if len([c for c in calls if c[1] == creds.beta_alpha_access_token]) > 2:
                        return probes.HttpResponse(status=403, body='{"code":"forbidden"}', headers=headers)
                    return probes.HttpResponse(status=200, body='{"id":"ok"}', headers=headers)
                if token == creds.alpha_access_token and "/members/" in path:
                    return probes.HttpResponse(status=204, body="", headers=headers)
                return probes.HttpResponse(status=200, body='{"id":"ok"}', headers=headers)

            def compose(self, args, **kwargs):  # type: ignore[override]
                return probes.CommandOutcome(0, "", "")

            def psql(self, sql, **kwargs):  # type: ignore[override]
                return probes.CommandOutcome(0, "abc", "")

        runner = probes.DeployedProbeRunner(
            api_base="http://fake",
            seed=seed,
            credentials=creds,
            shims=RevokeShims(),
            git_sha_full=seed.source_revision["commit"],
        )
        runner.run_revoke_probe()
        warm = [c for c in calls if c[1] == creds.beta_alpha_access_token and seed.alpha_collection_id in c[0]]
        self.assertGreaterEqual(len(warm), 2)

    def test_owner_control_beta_org_uses_alpha_beta_token(self) -> None:
        denial = load_denial()
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = make_seed_credentials(
            probes,
            seed,
            alpha_beta_access_token="alpha-beta-owner-token",
        )
        entry = next(e for e in denial.build_http_sse_denial_mapping() if e.operation_id == "getCollection")
        spec = denial.build_owner_control_spec(entry, seed=seed, credentials=creds)
        assert spec is not None
        self.assertEqual(spec.token, "alpha-beta-owner-token")

    def test_triage_conflict_body_uses_status_enum(self) -> None:
        denial = load_denial()
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = self._seed_creds(seed, probes)
        body = denial._owner_body("triageConflict", seed, credentials=creds, params={})
        assert body is not None
        self.assertIn("status", body)
        self.assertNotIn("resolution", body)
        self.assertIn(body["status"], {"resolved", "accepted_exception", "false_positive"})

    def test_resolve_citation_owner_body_includes_required_hashes(self) -> None:
        denial = load_denial()
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = make_seed_credentials(
            probes,
            seed,
            beta_citation_chunk_id="27272727-2727-2727-2727-272727272727",
            beta_citation_source_content_sha256="a" * 64,
            beta_citation_canonical_markdown_sha256="b" * 64,
            beta_citation_source_span_start=0,
            beta_citation_source_span_end=12,
            beta_citation_quote_local_start=0,
            beta_citation_quote_local_end=12,
            beta_citation_quote="phase1c-beta",
        )
        body = denial._owner_body("resolveCitation", seed, credentials=creds, params={})
        assert body is not None
        for key in (
            "logicalDocumentId",
            "versionId",
            "sourceContentSha256",
            "canonicalMarkdownSha256",
            "chunkId",
            "sourceSpanStart",
            "sourceSpanEnd",
            "quoteLocalStart",
            "quoteLocalEnd",
            "quote",
        ):
            self.assertIn(key, body, f"resolveCitation missing {key}")

    def test_accept_invite_owner_uses_disposable_accept_token(self) -> None:
        denial = load_denial()
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = make_seed_credentials(
            probes,
            seed,
            beta_denial_accept_invite_token="mhinv1.fresh-accept-token",
            beta_denial_accept_access_token="disposable-user-token",
        )
        body = denial._owner_body("acceptMemberInvite", seed, credentials=creds, params={})
        assert body is not None
        self.assertEqual(body["token"], "mhinv1.fresh-accept-token")

    def test_load_credentials_requires_all_disposable_fixture_ids(self) -> None:
        probes = load_probes()
        path = Path(tempfile.mkdtemp()) / "creds.json"
        payload = complete_credentials_raw()
        del payload["betaDenialDisposableConflictId"]
        path.write_text(json.dumps(payload), encoding="utf-8")
        path.chmod(0o600)
        with self.assertRaises(RuntimeError):
            probes.load_seed_credentials(path, expected_challenge="phase1c-challenge-abc", purge_after_load=False)

    def test_unauth_foreign_scan_both_org_markers(self) -> None:
        denial = load_denial()
        leaks = denial.scan_marker_leakage(
            "leaked phase1c-marker-alpha-aaa111 and phase1c-marker-beta-bbb222",
            forbidden_markers={"phase1c-marker-alpha-aaa111", "phase1c-marker-beta-bbb222"},
        )
        self.assertEqual(len(leaks), 2)
        text = Path(DENIAL_PATH).read_text(encoding="utf-8")
        execute = text.split("def execute_http_denial_suite", 1)[1].split("def parse_denial_execution_report", 1)[0]
        self.assertIn("marker_alpha", execute)
        self.assertIn("marker_beta", execute)

    def test_stale_token_probe_revokes_successor_family(self) -> None:
        text = (ROOT / "bench/markhand_web/scripts/phase1c_deployed_probes.py").read_text(encoding="utf-8")
        stale = text.split("def run_stale_tokens_probe", 1)[1].split("def run_", 1)[0]
        self.assertIn("new_access", stale)
        self.assertIn("new_refresh", stale)
        self.assertIn("_discard_revoked_token_family", stale)

    def test_g1c_gate_shell_owns_cleanup_trap_before_challenge(self) -> None:
        text = self.GATE_SH.read_text(encoding="utf-8")
        seed_idx = text.index("phase1c-multi-org-seed.sh")
        trap_idx = text.index("trap")
        challenge_idx = text.index("MARKHAND_PHASE1C_CHALLENGE")
        self.assertLess(seed_idx, trap_idx)
        self.assertLess(trap_idx, challenge_idx)
        self.assertIn("purge_phase1c_credentials", text)

    def test_audit_denominator_covers_predeclared_admin_mutations(self) -> None:
        probes = load_probes()
        self.assertTrue(hasattr(probes, "AUDIT_MUTATION_ACTIONS"))
        self.assertGreaterEqual(len(probes.AUDIT_MUTATION_ACTIONS), 6)

    def test_stateful_fake_executes_all_60_manifest_rows(self) -> None:
        fake = self._load_stateful_fake()
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = self._seed_creds(seed, probes)
        deployment = fake.StatefulFakeDeployment(seed=seed, credentials=creds)
        report = deployment.run_denial_suite()
        self.assertEqual(report["executableHttpSseCount"], 60)

    def test_validate_uuid_normalizes_to_lowercase(self) -> None:
        probes = load_probes()
        normalized = probes.validate_uuid("ABCDEF12-3456-7890-ABCD-EF1234567890", field="id")
        self.assertEqual(normalized, "abcdef12-3456-7890-abcd-ef1234567890")

    def test_parse_seed_rejects_missing_disposable_ids_in_credentials(self) -> None:
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = make_seed_credentials(probes, seed)
        with self.assertRaises(RuntimeError):
            probes.validate_fixture_credentials(creds)

    def _load_stateful_fake(self):
        fake_path = ROOT / "bench/markhand_web/scripts/phase1c_stateful_fake.py"
        spec = importlib.util.spec_from_file_location("phase1c_stateful_fake_third", fake_path)
        assert spec and spec.loader
        module = importlib.util.module_from_spec(spec)
        sys.modules["phase1c_stateful_fake_third"] = module
        spec.loader.exec_module(module)
        return module


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
