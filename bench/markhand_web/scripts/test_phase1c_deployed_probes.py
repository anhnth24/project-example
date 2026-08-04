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
        "disposableOrgId": "66666666-6666-6666-6666-666666666666",
        "alphaDuplicateCollectionId": "12121212-1212-1212-1212-121212121211",
        "betaDuplicateCollectionId": "23232323-2323-2323-2323-232323232322",
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
        "betaDenialDisposableDeleteMemberUserId": "77777777-7777-7777-7777-777777777701",
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
        "betaDenialNegativeInviteToken": "mhinv1.negative-token",
        "betaDenialWrongDownloadCapability": "mhcap1.wrong-token",
        "betaDenialStaleAccessToken": "stale-access-token-value",
        "betaDenialQuarantinedDocumentId": "28282828-2828-2828-2828-282828282828",
        "betaDenialQuarantinedCollectionId": "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
        "betaDenialAlreadyMemberInviteToken": "mhinv1.already-member-token",
        "betaCitationExpiredVersionId": "12121212-1212-1212-1212-121212121212",
        "betaDenialStaleAfterDowngradeToken": "stale-after-downgrade-token",
        "betaDenialStaleAfterRemoveToken": "stale-after-remove-token",
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
        "alpha_beta_access_token": "alpha-beta-token",
        "alpha_beta_refresh_token": "alpha-beta-refresh",
        "alpha_session_id": "11111111-1111-1111-1111-111111111101",
        "beta_session_id": "22222222-2222-2222-2222-222222222201",
        "beta_invite_token": "mhinv1.test-token",
        "alpha_download_capability": "cap-alpha-token",
        "beta_download_capability": "cap-beta-token",
        "beta_denial_disposable_collection_id": "21212121-2121-2121-2121-212121212121",
        "beta_denial_disposable_collection_update_id": "21212121-2121-2121-2121-212121212122",
        "beta_denial_disposable_document_id": "23232323-2323-2323-2323-232323232323",
        "beta_denial_disposable_chat_session_id": "24242424-2424-2424-2424-242424242424",
        "beta_denial_disposable_invite_id": "25252525-2525-2525-2525-252525252525",
        "beta_denial_disposable_member_user_id": "44444444-4444-4444-4444-444444444401",
        "beta_denial_disposable_delete_member_user_id": "77777777-7777-7777-7777-777777777701",
        "beta_denial_disposable_conflict_id": "26262626-2626-2626-2626-262626262626",
        "beta_denial_accept_invite_token": "mhinv1.accept-disposable-token",
        "beta_denial_accept_access_token": "disposable-accept-token",
        "beta_citation_chunk_id": "27272727-2727-2727-2727-272727272727",
        "beta_citation_source_content_sha256": "a" * 64,
        "beta_citation_canonical_markdown_sha256": "b" * 64,
        "beta_citation_source_span_start": 0,
        "beta_citation_source_span_end": 12,
        "beta_citation_quote_local_start": 0,
        "beta_citation_quote_local_end": 12,
        "beta_citation_quote": "phase1c-beta",
        "beta_denial_negative_invite_token": "mhinv1.22222222-2222-2222-2222-222222222222.0123456789abcdef",
        "beta_denial_wrong_download_capability": "mhcap1.wrong-token",
        "beta_denial_stale_access_token": "stale-access-token-value",
        "beta_denial_quarantined_document_id": "28282828-2828-2828-2828-282828282828",
        "beta_denial_quarantined_collection_id": "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
        "beta_denial_already_member_invite_token": "mhinv1.already-member-token",
        "beta_citation_expired_version_id": "12121212-1212-1212-1212-121212121212",
        "beta_denial_stale_after_downgrade_token": "stale-after-downgrade-token",
        "beta_denial_stale_after_remove_token": "stale-after-remove-token",
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

    def test_trivy_parser_requires_exact_artifact_name_match(self) -> None:
        probes = load_probes()
        digest = "sha256:" + "a" * 64
        ref = f"markhand-api:poc@{digest}"
        report = {
            "SchemaVersion": 2,
            "ArtifactName": ref,
            "Results": [{"Target": f"markhand-api:poc ({digest})", "Vulnerabilities": []}],
        }
        probes.validate_trivy_report_target(report, requested_ref=ref)
        with self.assertRaises(RuntimeError):
            probes.validate_trivy_report_target(
                {"SchemaVersion": 2, "ArtifactName": ref, "Results": []},
                requested_ref="markhand-api:poc@sha256:" + "b" * 64,
            )
        with self.assertRaises(RuntimeError):
            probes.validate_trivy_report_target(
                {"SchemaVersion": 2, "ArtifactName": f"other:{digest}", "Results": []},
                requested_ref=ref,
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
        creds = make_seed_credentials(
            probes,
            seed,
            alpha_access_token="tok-alpha",
            beta_access_token="tok-beta",
            alpha_beta_access_token="tok-beta-owner",
        )
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
        seed = probes.parse_seed_artifact(
            complete_seed_raw(probes),
            expected_challenge="phase1c-challenge-abc",
        )
        creds = make_seed_credentials(
            probes,
            seed,
            alpha_access_token="tok-alpha",
            beta_access_token="tok-beta",
            alpha_beta_access_token="tok-beta-owner",
        )

        class TrackingShims(probes.DeployedProbeShims):
            def __init__(self) -> None:
                fake_path = ROOT / "bench/markhand_web/scripts/phase1c_stateful_fake.py"
                spec = importlib.util.spec_from_file_location("phase1c_stateful_fake_track", fake_path)
                assert spec and spec.loader
                fake_mod = importlib.util.module_from_spec(spec)
                sys.modules[spec.name] = fake_mod
                spec.loader.exec_module(fake_mod)
                self._fake = fake_mod.StatefulFakeDeployment(seed=seed, credentials=creds)

            def http_request(self, **kwargs):  # type: ignore[override]
                journal.append(f"http:{kwargs.get('method')}:{kwargs.get('path')}")
                return self._fake._http_request(**kwargs)

            def compose(self, args, **kwargs):  # type: ignore[override]
                journal.append("compose:" + " ".join(args[:2]))
                return probes.CommandOutcome(exit_code=0, stdout="", stderr="")

            def psql(self, sql, **kwargs):  # type: ignore[override]
                journal.append("psql:" + sql[:40])
                return probes.CommandOutcome(exit_code=0, stdout="0", stderr="")

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
            invite_request_id = "55555555-5555-5555-5555-555555555501"
            accept_request_id = "66666666-6666-6666-6666-666666666601"
            switch_access = "77777777-7777-7777-7777-777777777701"
            switch_session = "88888888-8888-8888-8888-888888888801"
            accept_access = "99999999-9999-9999-9999-999999999901"

            def __init__(self) -> None:
                self._beta_refresh_reused = False

            def http_request(self, **kwargs):  # type: ignore[override]
                path = str(kwargs.get("path") or "")
                method = str(kwargs.get("method") or "GET").upper()
                if path.endswith("/api/v1/auth/login"):
                    return probes.HttpResponse(
                        status=200,
                        body=json.dumps(
                            {
                                "accessToken": self.switch_access,
                                "refreshToken": "admin-refresh",
                                "requestId": self.accept_request_id,
                            }
                        ),
                        headers={"x-request-id": self.accept_request_id},
                    )
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
                if path.endswith("/api/v1/auth/me") and kwargs.get("token") == self.accept_access:
                    return probes.HttpResponse(
                        status=200,
                        body=json.dumps(
                            {
                                "userId": "55555555-5555-5555-5555-555555555501",
                                "orgId": seed.org_alpha_id,
                                "sessionId": self.switch_session,
                                "requestId": self.accept_request_id,
                            }
                        ),
                        headers={"x-request-id": self.accept_request_id},
                    )
                if path.endswith("/api/v1/collections") and method == "POST":
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
                if path.endswith("/api/v1/members/invites") and method == "POST":
                    invite_id = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb"
                    return probes.HttpResponse(
                        status=201,
                        body=json.dumps(
                            {
                                "invite": {"id": invite_id},
                                "token": "mhinv1.audit-invite-token",
                                "requestId": self.invite_request_id,
                            }
                        ),
                        headers={"x-request-id": self.invite_request_id},
                    )
                if path.endswith("/api/v1/members/invites/accept") and method == "POST":
                    return probes.HttpResponse(
                        status=201,
                        body=json.dumps({"userId": "55555555-5555-5555-5555-555555555501", "requestId": self.accept_request_id}),
                        headers={"x-request-id": self.accept_request_id},
                    )
                if "/members/invites/" in path and path.endswith("/revoke"):
                    return probes.HttpResponse(
                        status=204,
                        body="",
                        headers={"x-request-id": "12121212-1212-1212-1212-121212121212"},
                    )
                if path.startswith("/api/v1/members/") and method == "PATCH":
                    return probes.HttpResponse(
                        status=200,
                        body=json.dumps({"requestId": "13131313-1313-1313-1313-131313131313"}),
                        headers={"x-request-id": "13131313-1313-1313-1313-131313131313"},
                    )
                if path.startswith("/api/v1/members/") and method == "DELETE":
                    return probes.HttpResponse(
                        status=204,
                        body="",
                        headers={"x-request-id": "14141414-1414-1414-1414-141414141414"},
                    )
                if path.endswith("/api/v1/auth/refresh"):
                    refresh = (kwargs.get("body") or {}).get("refreshToken")
                    if refresh == creds.beta_refresh_token:
                        if self._beta_refresh_reused:
                            return probes.HttpResponse(
                                status=401,
                                body='{"code":"unauthorized"}',
                                headers={"x-request-id": "15151515-1515-1515-1515-151515151515"},
                            )
                        self._beta_refresh_reused = True
                        return probes.HttpResponse(
                            status=200,
                            body=json.dumps(
                                {
                                    "accessToken": "rotated-access",
                                    "refreshToken": "rotated-refresh",
                                    "requestId": "16161616-1616-1616-1616-161616161616",
                                }
                            ),
                            headers={"x-request-id": "16161616-1616-1616-1616-161616161616"},
                        )
                    return probes.HttpResponse(status=401, body='{"code":"unauthorized"}', headers={})
                if path.endswith("/api/v1/auth/logout"):
                    return probes.HttpResponse(
                        status=204,
                        body="",
                        headers={"x-request-id": "17171717-1717-1717-1717-171717171717"},
                    )
                if path.endswith("/triage") and method == "POST":
                    return probes.HttpResponse(
                        status=200,
                        body=json.dumps(
                            {
                                "id": seed.alpha_conflict_id,
                                "status": "resolved",
                                "requestId": "18181818-1818-1818-1818-181818181818",
                            }
                        ),
                        headers={"x-request-id": "18181818-1818-1818-1818-181818181818"},
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
                                },
                                {
                                    "id": "77777777-7777-7777-7777-777777777702",
                                    "actorId": seed.alpha_user_id,
                                    "action": "collection.create",
                                    "targetType": "collection",
                                    "targetId": "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
                                    "outcome": "success",
                                    "requestId": self.create_request_id,
                                    "occurredAt": "2026-08-04T12:00:01Z",
                                },
                                {
                                    "id": "77777777-7777-7777-7777-777777777703",
                                    "actorId": seed.alpha_user_id,
                                    "action": "member.invite",
                                    "targetType": "member",
                                    "targetId": "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
                                    "outcome": "success",
                                    "requestId": self.invite_request_id,
                                    "occurredAt": "2026-08-04T12:00:02Z",
                                },
                                {
                                    "id": "77777777-7777-7777-7777-777777777704",
                                    "actorId": "55555555-5555-5555-5555-555555555501",
                                    "action": "member.invite_accept",
                                    "targetType": "member",
                                    "targetId": "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
                                    "outcome": "success",
                                    "requestId": self.accept_request_id,
                                    "occurredAt": "2026-08-04T12:00:03Z",
                                },
                                {
                                    "id": "77777777-7777-7777-7777-777777777705",
                                    "actorId": seed.alpha_user_id,
                                    "action": "member.invite_revoke",
                                    "targetType": "member",
                                    "targetId": "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
                                    "outcome": "success",
                                    "requestId": "12121212-1212-1212-1212-121212121212",
                                    "occurredAt": "2026-08-04T12:00:04Z",
                                },
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
        negative_rows = {
            spec.row_id for spec in specs if spec.scenario not in {"owner_control", "unauthenticated"}
        }
        owner_rows = {spec.row_id for spec in specs if spec.scenario == "owner_control"}
        self.assertTrue(owner_rows, "owner_control scenarios required")
        self.assertEqual(
            negative_rows,
            owner_rows,
            "every negative denial row must have a matching owner_control warm-up",
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
        payload = complete_credentials_raw()
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
        self.assertGreater(report["ownerControlCount"], 0)
        self.assertGreater(report["foreignCount"], report["ownerControlCount"])
        self.assertEqual(report["executableHttpSseCount"], 60)

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
        self.assertTrue(denial._uses_foreign_scope("createUpload", "/uploads"))

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
        self.assertIn("_claim_pair_for_conflict", text)
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
            _editor: bool = True

            def http_request(self, **kwargs):  # type: ignore[override]
                calls.append((str(kwargs.get("method")), str(kwargs.get("path")), kwargs.get("token")))
                headers = {"x-request-id": "11111111-1111-1111-1111-111111111111"}
                token = kwargs.get("token")
                if kwargs.get("multipart_body") is not None and token == creds.beta_alpha_access_token:
                    if not self._editor:
                        return probes.HttpResponse(status=403, body='{"code":"forbidden"}', headers=headers)
                    return probes.HttpResponse(status=201, body='{"documentId":"d","versionId":"v"}', headers=headers)
                if token == creds.alpha_access_token and kwargs.get("method") == "PATCH":
                    role = (kwargs.get("body") or {}).get("role")
                    if role == "viewer":
                        self._editor = False
                    elif role == "editor":
                        self._editor = True
                    return probes.HttpResponse(status=200, body="{}", headers=headers)
                if token == creds.beta_alpha_access_token:
                    return probes.HttpResponse(status=200, body='{"userId":"u","sessionId":"s"}', headers=headers)
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
        seed_idx = text.index("bash deploy/scripts/phase1c-multi-org-seed.sh")
        trap_idx = text.index("trap '")
        challenge_idx = text.index("MARKHAND_PHASE1C_CHALLENGE")
        self.assertLess(trap_idx, seed_idx)
        self.assertLess(seed_idx, challenge_idx)
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
        creds = make_seed_credentials(probes, seed, beta_denial_disposable_conflict_id="")
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


class Phase1cFourthReviewSliceTests(unittest.TestCase):
    """Commit O RED: fourth-review bounded HTTP slice contracts."""

    SEED_PY = ROOT / "bench/markhand_web/scripts/phase1c_multi_org_seed.py"
    GATE_SH = ROOT / "deploy/scripts/g1c-security-gate.sh"
    DENIAL_PY = DENIAL_PATH
    PROBES_PY = PROBES_PATH
    FAKE_PY = ROOT / "bench/markhand_web/scripts/phase1c_stateful_fake.py"

    def _seed_fixture(self, probes):
        return probes.parse_seed_artifact(
            complete_seed_raw(probes),
            expected_challenge="phase1c-challenge-abc",
        )

    def _seed_creds(self, seed, probes):
        return make_seed_credentials(probes, seed)

    def test_citation_fixture_queries_document_versions_and_derived_artifacts(self) -> None:
        text = self.SEED_PY.read_text(encoding="utf-8")
        self.assertIn("artifact_kind = 'markdown'", text)
        self.assertIn("content_sha256", text)
        self.assertNotIn("canonical_markdown_sha256 FROM document_versions", text)
        self.assertNotIn("source_content_sha256 FROM document_versions", text)

    def test_citation_fixture_waits_for_indexing_with_bounded_poll(self) -> None:
        text = self.SEED_PY.read_text(encoding="utf-8")
        self.assertIn("_wait_for_citation_index", text)

    def test_bootstrap_beta_as_alpha_org_editor(self) -> None:
        text = self.SEED_PY.read_text(encoding="utf-8")
        self.assertIn("'editor'", text)
        self.assertNotIn(
            "VALUES ('{ALPHA_ORG_ID}', '{BETA_USER_ID}', 'viewer', 'active')",
            text.replace(" ", ""),
        )

    def test_conflict_claim_pair_unique_per_conflict_slot(self) -> None:
        text = self.SEED_PY.read_text(encoding="utf-8")
        self.assertIn("_claim_pair_for_conflict", text)
        self.assertNotIn("_claim_pair_for_org(org_id", text)

    def test_seed_provisions_disposable_org_without_alpha_membership(self) -> None:
        text = self.SEED_PY.read_text(encoding="utf-8")
        self.assertIn("disposableOrgId", text)

    def test_row_scenario_handlers_cover_all_secondary_rows(self) -> None:
        denial = load_denial()
        self.assertTrue(hasattr(denial, "ROW_SCENARIO_HANDLERS"))
        for row_id in denial.SECONDARY_ROW_IDS:
            handler = denial.ROW_SCENARIO_HANDLERS.get(row_id)
            self.assertIsNotNone(handler, f"missing dedicated handler for {row_id}")
            self.assertNotEqual(
                handler.__name__,
                "_build_primary_row_specs",
                f"{row_id} must not use generic primary handler",
            )

    def test_denial_has_no_query_suffix_secondary_variants(self) -> None:
        denial = load_denial()
        self.assertFalse(hasattr(denial, "_variant_query_suffix"))

    def test_switch_org_negative_uses_disposable_org_membership_missing(self) -> None:
        denial = load_denial()
        probes = load_probes()
        seed = probes.parse_seed_artifact(
            complete_seed_raw(probes, disposableOrgId="66666666-6666-6666-6666-666666666666"),
            expected_challenge="phase1c-challenge-abc",
        )
        creds = make_seed_credentials(probes, seed)
        entry = next(e for e in denial.build_http_sse_denial_mapping() if e.row_id == "denial-switchOrg")
        specs = denial.build_row_denial_specs(entry, seed=seed, credentials=creds)
        negative = [s for s in specs if s.scenario == "membership_missing"]
        self.assertEqual(len(negative), 1)
        self.assertEqual(negative[0].expected_statuses, frozenset({403}))
        body = negative[0].body
        assert body is not None
        self.assertEqual(body["orgId"], seed.disposable_org_id)

    def test_redeem_download_negative_uses_invalid_capability_not_foreign_bearer(self) -> None:
        denial = load_denial()
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = make_seed_credentials(
            probes,
            seed,
            beta_denial_wrong_download_capability="mhcap1.invalid-token",
        )
        entry = next(e for e in denial.build_http_sse_denial_mapping() if e.operation_id == "redeemDownload")
        specs = denial.build_row_denial_specs(entry, seed=seed, credentials=creds)
        invalid = [s for s in specs if s.scenario == "invalid_capability"]
        self.assertEqual(len(invalid), 1)
        self.assertEqual(invalid[0].expected_statuses, frozenset({400}))
        self.assertIn("invalid-token", invalid[0].path)
        self.assertNotEqual(invalid[0].token, creds.alpha_access_token)

    def test_accept_invite_negative_uses_separate_invite_token(self) -> None:
        denial = load_denial()
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = make_seed_credentials(
            probes,
            seed,
            beta_denial_accept_invite_token="mhinv1.owner-accept-token",
            beta_denial_negative_invite_token="mhinv1.negative-invite-token",
        )
        entry = next(e for e in denial.build_http_sse_denial_mapping() if e.operation_id == "acceptMemberInvite")
        specs = denial.build_row_denial_specs(entry, seed=seed, credentials=creds)
        owner = next(s for s in specs if s.scenario == "owner_control")
        negative = next(s for s in specs if s.scenario == "invalid_invite_token")
        assert owner.body is not None and negative.body is not None
        self.assertEqual(owner.body["token"], "mhinv1.owner-accept-token")
        self.assertEqual(negative.body["token"], "mhinv1.negative-invite-token")
        self.assertNotEqual(owner.body["token"], negative.body["token"])
        self.assertEqual(negative.expected_statuses, frozenset({400}))

    def test_validate_fixture_credentials_requires_distinct_disposable_ids(self) -> None:
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = make_seed_credentials(
            probes,
            seed,
            beta_denial_disposable_collection_id="21212121-2121-2121-2121-212121212121",
            beta_denial_disposable_collection_update_id="21212121-2121-2121-2121-212121212121",
        )
        with self.assertRaises(RuntimeError):
            probes.validate_fixture_credentials(creds)

    def test_ensure_beta_membership_verifies_editor_role_via_upload(self) -> None:
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = self._seed_creds(seed, probes)

        class MembershipShim(probes.DeployedProbeShims):
            upload_calls = 0

            def http_request(self, **kwargs):  # type: ignore[override]
                path = str(kwargs.get("path") or "")
                token = kwargs.get("token")
                headers = {"x-request-id": "11111111-1111-1111-1111-111111111111"}
                if kwargs.get("multipart_body") is not None and token == creds.beta_alpha_access_token:
                    self.upload_calls += 1
                    if self.upload_calls == 1:
                        return probes.HttpResponse(status=403, body='{"code":"forbidden"}', headers=headers)
                    return probes.HttpResponse(status=201, body='{"documentId":"d","versionId":"v"}', headers=headers)
                if token == creds.alpha_access_token and kwargs.get("method") == "PATCH":
                    return probes.HttpResponse(status=200, body='{"id":"ok"}', headers=headers)
                if path.endswith("/api/v1/auth/me"):
                    return probes.HttpResponse(status=200, body='{"userId":"u","sessionId":"s"}', headers=headers)
                return probes.HttpResponse(status=403, body="{}", headers=headers)

            def compose(self, args, **kwargs):  # type: ignore[override]
                return probes.CommandOutcome(0, "", "")

            def psql(self, sql, **kwargs):  # type: ignore[override]
                return probes.CommandOutcome(0, "abc", "")

        runner = probes.DeployedProbeRunner(
            api_base="http://fake",
            seed=seed,
            credentials=creds,
            shims=MembershipShim(),
            git_sha_full=seed.source_revision["commit"],
        )
        runner._ensure_beta_membership()
        self.assertGreaterEqual(runner.shims.upload_calls, 2)  # type: ignore[attr-defined]

    def test_audit_probe_executes_all_predeclared_mutation_actions(self) -> None:
        probes = load_probes()
        self.assertGreaterEqual(len(probes.AUDIT_MUTATION_ACTIONS), 10)
        text = self.PROBES_PY.read_text(encoding="utf-8")
        audit = text.split("def run_audit_probe", 1)[1].split("def run_", 1)[0]
        for action in probes.AUDIT_MUTATION_ACTIONS:
            self.assertIn(action, audit, f"audit probe must attempt {action}")

    def test_load_seed_credentials_secure_uses_o_nofollow(self) -> None:
        probes = load_probes()
        self.assertTrue(hasattr(probes, "load_seed_credentials_secure"))

    def test_g1c_gate_trap_before_seed_invocation(self) -> None:
        text = self.GATE_SH.read_text(encoding="utf-8")
        seed_idx = text.index("bash deploy/scripts/phase1c-multi-org-seed.sh")
        trap_idx = text.index("trap '")
        self.assertLess(trap_idx, seed_idx)

    def test_sse_parser_validates_terminal_event(self) -> None:
        denial = load_denial()
        self.assertTrue(hasattr(denial, "parse_sse_stream"))
        with self.assertRaises(RuntimeError):
            denial.parse_sse_stream("event: partial\ndata: {}\n", required_terminal="done")

    def test_stateful_fake_has_no_generic_owner_fallback(self) -> None:
        text = self.FAKE_PY.read_text(encoding="utf-8")
        self.assertNotIn('if method in {"POST", "PATCH", "DELETE", "GET"}:', text)

    def test_denial_observation_includes_owner_transition(self) -> None:
        denial = load_denial()
        obs = denial.DenialObservation(
            operation_id="deleteCollection",
            row_id="denial-deleteCollection",
            scenario="owner_control",
            expected_statuses=[204],
            actual_status=204,
            body_sha256="abc",
            request_id="11111111-1111-1111-1111-111111111111",
            leaked_markers=[],
            owner_transition="collection_deleted",
        )
        payload = denial.DenialExecutionReport(
            schema_version=1,
            git_sha_full="a" * 40,
            manifest_sha256=denial.canonical_manifest_sha256(),
            challenge="c1",
            executable_http_sse_count=1,
            observations=[obs],
        ).as_dict()
        self.assertEqual(
            payload["observations"][0]["ownerTransition"],
            "collection_deleted",
        )

    def test_build_row_denial_specs_covers_all_sixty_rows(self) -> None:
        denial = load_denial()
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = make_seed_credentials(
            probes,
            seed,
            beta_denial_wrong_download_capability="mhcap1.invalid",
            beta_denial_negative_invite_token="mhinv1.negative",
        )
        mapping = denial.build_http_sse_denial_mapping()
        for entry in mapping:
            specs = denial.build_row_denial_specs(entry, seed=seed, credentials=creds)
            self.assertTrue(specs, f"{entry.row_id} produced no specs")


class Phase1cFifthReviewSliceTests(unittest.TestCase):
    """Commit Q RED: fifth-review bounded HTTP slice contracts."""

    SEED_PY = ROOT / "bench/markhand_web/scripts/phase1c_multi_org_seed.py"
    GATE_PY = GATE_PATH
    DENIAL_PY = DENIAL_PATH
    PROBES_PY = PROBES_PATH
    FAKE_PY = ROOT / "bench/markhand_web/scripts/phase1c_stateful_fake.py"

    def _seed_fixture(self, probes):
        return probes.parse_seed_artifact(
            complete_seed_raw(probes),
            expected_challenge="phase1c-challenge-abc",
        )

    def _seed_creds(self, seed, probes):
        return make_seed_credentials(probes, seed)

    def test_http_contract_schema_table_covers_all_owner_get_operations(self) -> None:
        denial = load_denial()
        table = denial.HTTP_OWNER_SUCCESS_SCHEMA
        mapping = denial.build_http_sse_denial_mapping()
        get_ops = {
            entry.operation_id
            for entry in mapping
            if entry.method.upper() == "GET"
            and entry.operation_id not in {"redeemDownload", "jobEvents"}
            and entry.layer != "sse"
        }
        missing = sorted(get_ops - set(table))
        self.assertFalse(missing, f"HTTP_OWNER_SUCCESS_SCHEMA missing GET operations: {missing}")

    def test_diff_document_versions_owner_path_includes_against_query(self) -> None:
        denial = load_denial()
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = self._seed_creds(seed, probes)
        entry = next(e for e in denial.build_http_sse_denial_mapping() if e.operation_id == "diffDocumentVersions")
        spec = denial.build_owner_control_spec(entry, seed=seed, credentials=creds)
        assert spec is not None
        self.assertIn("against=", spec.path)
        self.assertIn(seed.beta_version_id, spec.path)

    def test_get_usage_owner_schema_requires_items(self) -> None:
        denial = load_denial()
        self.assertEqual(denial.HTTP_OWNER_SUCCESS_SCHEMA["getUsage"], frozenset({"items"}))

    def test_get_document_version_schema_matches_openapi(self) -> None:
        denial = load_denial()
        keys = denial.HTTP_OWNER_SUCCESS_SCHEMA["getDocumentVersion"]
        for required in ("id", "documentId", "versionNumber", "isCurrent", "sourceContentSha256"):
            self.assertIn(required, keys)

    def test_diff_document_versions_schema_matches_openapi(self) -> None:
        denial = load_denial()
        self.assertEqual(
            denial.HTTP_OWNER_SUCCESS_SCHEMA["diffDocumentVersions"],
            frozenset({"documentId", "left", "right", "note", "requestId"}),
        )

    def test_load_seed_credentials_secure_reads_without_loaded_copy(self) -> None:
        probes = load_probes()
        secure_fn = self.PROBES_PY.read_text(encoding="utf-8").split("def load_seed_credentials_secure", 1)[1].split(
            "\ndef ", 1
        )[0]
        self.assertNotIn(".loaded", secure_fn)
        path = Path(tempfile.mkdtemp()) / "creds.json"
        payload = complete_credentials_raw()
        path.write_text(json.dumps(payload), encoding="utf-8")
        path.chmod(0o600)
        creds = probes.load_seed_credentials_secure(
            path, expected_challenge="phase1c-challenge-abc", purge_after_load=False
        )
        self.assertEqual(creds.alpha_access_token, "alpha-token")
        self.assertFalse(path.with_suffix(path.suffix + ".loaded").exists())

    def test_gate_build_context_uses_secure_credential_loader(self) -> None:
        text = self.GATE_PY.read_text(encoding="utf-8")
        ctx = text.split("def build_deployed_context", 1)[1].split("\ndef ", 1)[0]
        self.assertIn("load_seed_credentials_secure", ctx)
        self.assertNotIn("load_seed_credentials(", ctx)

    def test_unrestricted_loader_not_imported_on_gate_credential_path(self) -> None:
        text = self.GATE_PY.read_text(encoding="utf-8")
        ctx = text.split("def build_deployed_context", 1)[1].split("\ndef ", 1)[0]
        self.assertNotIn("load_seed_credentials(", ctx)

    def test_sse_qualifying_paths_require_stream_closed_terminal(self) -> None:
        denial = load_denial()
        self.assertEqual(denial.SSE_TERMINAL_EVENT, "stream.closed")
        envelope = self.DENIAL_PY.read_text(encoding="utf-8").split("def _validate_sse_envelope", 1)[1].split(
            "\ndef ", 1
        )[0]
        self.assertIn("SSE_TERMINAL_EVENT", envelope)

    def test_owner_transition_follow_up_covers_member_and_invite_mutations(self) -> None:
        denial = load_denial()
        fn = self.DENIAL_PY.read_text(encoding="utf-8").split("def _validate_owner_transition", 1)[1].split(
            "\ndef ", 1
        )[0]
        for transition in (
            "member_role_updated",
            "member_deleted",
            "invite_revoked",
            "invite_accepted",
            "collection_updated",
        ):
            self.assertIn(transition, fn)

    def test_stale_token_secondary_uses_stale_access_credential(self) -> None:
        denial = load_denial()
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = make_seed_credentials(
            probes,
            seed,
            beta_denial_stale_access_token="stale-access-token-value",
            beta_denial_stale_after_downgrade_token="stale-downgrade-token",
            beta_denial_stale_after_remove_token="stale-remove-token",
        )
        entry = next(e for e in denial.build_http_sse_denial_mapping() if e.row_id == "denial-task13-stale-tokens")
        specs = denial.build_row_denial_specs(entry, seed=seed, credentials=creds)
        stale = [s for s in specs if s.scenario.startswith("stale_token")]
        self.assertGreaterEqual(len(stale), 1)
        self.assertEqual(stale[0].token, "stale-access-token-value")
        self.assertEqual(stale[0].expected_statuses, frozenset({401}))

    def test_seed_reconciles_identity_fixtures_before_invites(self) -> None:
        text = self.SEED_PY.read_text(encoding="utf-8")
        self.assertIn("_reconcile_identity_fixtures", text)
        reconcile = text.split("def _reconcile_identity_fixtures", 1)[1].split("\ndef ", 1)[0]
        self.assertIn("member_invites", reconcile)
        self.assertIn("org_memberships", reconcile)

    def test_audit_logout_body_includes_refresh_token(self) -> None:
        text = self.PROBES_PY.read_text(encoding="utf-8")
        audit = text.split("def run_audit_probe", 1)[1].split("\n    def run_", 1)[0]
        self.assertIn("refreshToken", audit)
        self.assertIn("/api/v1/auth/logout", audit)

    def test_stateful_fake_raises_on_unknown_owner_mapping(self) -> None:
        fake_path = self.FAKE_PY
        spec = importlib.util.spec_from_file_location("phase1c_stateful_fake_fifth", fake_path)
        assert spec and spec.loader
        module = importlib.util.module_from_spec(spec)
        sys.modules["phase1c_stateful_fake_fifth"] = module
        spec.loader.exec_module(module)
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = self._seed_creds(seed, probes)
        deployment = module.StatefulFakeDeployment(seed=seed, credentials=creds)
        with self.assertRaises(RuntimeError):
            deployment._http_request(
                method="GET",
                path="/api/v1/unknown-resource",
                token=creds.alpha_beta_access_token,
            )

    def test_contract_table_test_generated_from_fixture_requirements(self) -> None:
        denial = load_denial()
        self.assertTrue(hasattr(denial, "validate_http_contract_schema_table"))
        denial.validate_http_contract_schema_table()


class Phase1cSixthReviewSliceTests(unittest.TestCase):
    """Commit S RED: sixth-review bounded HTTP slice contracts."""

    SEED_PY = ROOT / "bench/markhand_web/scripts/phase1c_multi_org_seed.py"
    DENIAL_PY = DENIAL_PATH
    PROBES_PY = PROBES_PATH
    FAKE_PY = ROOT / "bench/markhand_web/scripts/phase1c_stateful_fake.py"

    def _seed_fixture(self, probes):
        return probes.parse_seed_artifact(
            complete_seed_raw(probes),
            expected_challenge="phase1c-challenge-abc",
        )

    def _seed_creds(self, seed, probes):
        return make_seed_credentials(probes, seed)

    def test_get_chat_session_schema_is_flattened_session_fields(self) -> None:
        denial = load_denial()
        self.assertEqual(
            denial.HTTP_OWNER_SUCCESS_SCHEMA["getChatSession"],
            frozenset({"id", "title", "turns"}),
        )
        self.assertTrue(hasattr(denial, "validate_owner_read_response"))

    def test_owner_transition_uses_list_members_not_get_member_by_id(self) -> None:
        fn = self.DENIAL_PY.read_text(encoding="utf-8").split("def _validate_owner_transition", 1)[1].split(
            "\ndef ", 1
        )[0]
        self.assertIn("/api/v1/members", fn)
        self.assertNotIn("/api/v1/members/{member_id}", fn)
        self.assertNotIn('f"{API_PREFIX}/members/{member_id}"', fn)
        self.assertNotIn('f"{API_PREFIX}/members/{delete_member_id}"', fn)
        self.assertNotIn('f"{API_PREFIX}/members/{disposable_member}"', fn)

    def test_negative_invite_token_uses_valid_mhinv1_syntax(self) -> None:
        text = self.SEED_PY.read_text(encoding="utf-8")
        negative = text.split("beta_denial_negative_invite_token", 1)[1].split("\n", 8)[0:8]
        joined = "\n".join(negative)
        self.assertIn("mhinv1.", joined)
        self.assertIn("org_beta_id", joined)
        self.assertNotIn("phase1c-negative-", joined)

    def test_seed_bootstrap_memberships_after_final_reconcile(self) -> None:
        text = self.SEED_PY.read_text(encoding="utf-8")
        reconcile = text.index("_reconcile_identity_fixtures(org_ids=[ALPHA_ORG_ID, org_beta_id]")
        bootstrap = text.index("_bootstrap_identity_users(password_hash", reconcile)
        self.assertLess(reconcile, bootstrap)
        login_block = text.split("accept_login = _login(api_base, ACCEPT_EMAIL", 1)[0]
        self.assertIn("_bootstrap_identity_users", login_block)

    def test_seed_provisions_three_independent_invite_controls(self) -> None:
        text = self.SEED_PY.read_text(encoding="utf-8")
        self.assertIn("REVOKE_CONTROL_EMAIL", text)
        self.assertIn("beta_denial_accept_invite_token", text)
        self.assertIn("beta_denial_disposable_invite_id", text)
        self.assertIn("beta_denial_negative_invite_token", text)
        accept_block = text.split("beta_denial_accept_invite_token =", 1)[1].split("\n\n", 1)[0]
        self.assertNotIn("invites/accept", accept_block)
        revoke_block = text.split("beta_denial_disposable_invite_id =", 1)[1].split("\n\n", 1)[0]
        self.assertNotIn("invites/accept", revoke_block)

    def test_duplicate_names_secondary_proves_owner_ids_and_foreign_denial(self) -> None:
        denial = load_denial()
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = self._seed_creds(seed, probes)
        entry = next(e for e in denial.build_http_sse_denial_mapping() if e.row_id == "denial-task13-duplicate-names")
        specs = denial.build_row_denial_specs(entry, seed=seed, credentials=creds)
        scenarios = {spec.scenario for spec in specs}
        self.assertIn("duplicate_name_owner_lookup", scenarios)
        self.assertIn("duplicate_name_foreign_oracle", scenarios)

    def test_citation_secondary_has_replay_expired_and_mismatch_scenarios(self) -> None:
        denial = load_denial()
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = self._seed_creds(seed, probes)
        entry = next(e for e in denial.build_http_sse_denial_mapping() if e.row_id == "denial-resolveCitation-citation")
        specs = denial.build_row_denial_specs(entry, seed=seed, credentials=creds)
        scenarios = {spec.scenario for spec in specs}
        self.assertIn("citation_repeat", scenarios)
        self.assertTrue(any(getattr(s, "coverage_limited", False) for s in specs if s.scenario == "citation_repeat"))
        self.assertIn("citation_expired", scenarios)
        self.assertIn("citation_mismatch", scenarios)

    def test_preview_download_sse_secondary_executes_endpoint_sequence(self) -> None:
        denial = load_denial()
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = self._seed_creds(seed, probes)
        entry = next(
            e for e in denial.build_http_sse_denial_mapping() if e.row_id == "denial-task13-preview-download-sse"
        )
        specs = denial.build_row_denial_specs(entry, seed=seed, credentials=creds)
        ops = {spec.operation_id for spec in specs if spec.scenario.startswith("preview_download_")}
        for required in ("previewDocument", "issueDownloadCapability", "redeemDownload", "getJob", "jobEvents"):
            self.assertIn(required, ops)

    def test_in_flight_secondary_has_dedicated_evidence_transitions(self) -> None:
        denial = load_denial()
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = self._seed_creds(seed, probes)
        entry = next(
            e for e in denial.build_http_sse_denial_mapping() if e.row_id == "denial-task13-in-flight-ask-revoke"
        )
        specs = denial.build_row_denial_specs(entry, seed=seed, credentials=creds)
        transitions = {spec.owner_transition for spec in specs if spec.owner_transition}
        self.assertIn("ask_stream_started", transitions)
        self.assertIn("ask_stream_revoked", transitions)

    def test_audit_invite_accept_target_is_invite_id(self) -> None:
        audit = self.PROBES_PY.read_text(encoding="utf-8").split("def run_audit_probe", 1)[1].split("\n    def run_", 1)[0]
        self.assertIn('"targetId": invite_id', audit)
        self.assertNotIn('"targetId": accept_user_id', audit)

    def test_audit_refresh_reuse_outcome_is_deny(self) -> None:
        audit = self.PROBES_PY.read_text(encoding="utf-8").split("def run_audit_probe", 1)[1].split("\n    def run_", 1)[0]
        self.assertIn('"outcome": "deny"', audit)
        self.assertNotIn('"outcome": "failure" if reuse.status == 401 else "success"', audit)

    def test_patch_member_owner_toggles_editor_to_viewer(self) -> None:
        denial = load_denial()
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = self._seed_creds(seed, probes)
        body = denial._owner_body(
            "patchMember",
            seed,
            credentials=creds,
            params={"userId": creds.beta_denial_disposable_member_user_id},
        )
        assert body is not None
        self.assertEqual(body["role"], "viewer")

    def test_reconcile_identity_scoped_to_fixture_org_ids(self) -> None:
        text = self.SEED_PY.read_text(encoding="utf-8")
        reconcile = text.split("def _reconcile_identity_fixtures", 1)[1].split("\ndef ", 1)[0]
        self.assertIn("org_id IN", reconcile)
        self.assertIn("ALPHA_ORG_ID", reconcile)
        self.assertIn("org_beta_id", reconcile)

    def test_purge_credentials_uses_dir_fd_unlink(self) -> None:
        fn = self.PROBES_PY.read_text(encoding="utf-8").split("def purge_phase1c_credentials", 1)[1].split("\ndef ", 1)[0]
        self.assertIn("dir_fd", fn)
        self.assertIn("os.unlink", fn)
        self.assertNotIn(".write(b\"\\x00\"", fn)
        self.assertNotIn("path.stat()", fn)

    def test_sse_parser_validates_production_envelope(self) -> None:
        denial = load_denial()
        self.assertTrue(hasattr(denial, "validate_sse_envelope"))
        sample = (
            "id: 1\n"
            "event: ask.token\n"
            'data: {"version":1,"sequence":1,"event":"ask.token","requestId":"11111111-1111-1111-1111-111111111111","data":{}}\n\n'
            "id: 2\n"
            "event: stream.closed\n"
            'data: {"version":1,"sequence":2,"event":"stream.closed","requestId":"11111111-1111-1111-1111-111111111111","data":{"reason":"done"}}\n\n'
        )
        denial.validate_sse_envelope(sample, operation_id="askStream", headers={"content-type": "text/event-stream"})

    def test_stateful_fake_raises_for_unknown_http_method(self) -> None:
        fake = self._load_stateful_fake()
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = self._seed_creds(seed, probes)
        deployment = fake.StatefulFakeDeployment(seed=seed, credentials=creds)
        with self.assertRaises(RuntimeError):
            deployment._http_request(
                method="OPTIONS",
                path="/api/v1/collections",
                token=creds.alpha_beta_access_token,
            )

    def test_stateful_fake_has_no_generic_post_patch_delete_fallback(self) -> None:
        text = self.FAKE_PY.read_text(encoding="utf-8")
        self.assertNotIn("if method in {\"POST\", \"PATCH\", \"DELETE\"}:", text)
        self.assertNotIn('payload[key] = self.seed.beta_collection_id if key.endswith("Id") else "ok"', text)

    def test_validate_owner_read_response_per_operation(self) -> None:
        denial = load_denial()
        self.assertTrue(callable(getattr(denial, "validate_owner_read_response", None)))
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = self._seed_creds(seed, probes)
        body = json.dumps(
            {
                "id": seed.beta_chat_session_id,
                "title": "phase1c-owner-chat",
                "createdAt": "2026-08-04T00:00:00Z",
                "updatedAt": "2026-08-04T00:00:00Z",
                "turns": [],
            }
        )
        denial.validate_owner_read_response(
            "getChatSession",
            body,
            seed=seed,
            credentials=creds,
            path=f"/api/v1/chat-sessions/{seed.beta_chat_session_id}",
        )

    def _load_stateful_fake(self):
        spec = importlib.util.spec_from_file_location("phase1c_stateful_fake_sixth", self.FAKE_PY)
        assert spec and spec.loader
        module = importlib.util.module_from_spec(spec)
        sys.modules["phase1c_stateful_fake_sixth"] = module
        spec.loader.exec_module(module)
        return module


class Phase1cSemanticsSliceTests(unittest.TestCase):
    """Task 16 semantics: production-correct HTTP/probe contracts."""

    FAKE_PY = ROOT / "bench/markhand_web/scripts/phase1c_stateful_fake.py"

    def _seed_fixture(self, probes):
        return probes.parse_seed_artifact(
            complete_seed_raw(probes),
            expected_challenge="phase1c-challenge-abc",
        )

    def _seed_creds(self, seed, probes):
        return make_seed_credentials(probes, seed)

    def _load_stateful_fake(self):
        spec = importlib.util.spec_from_file_location("phase1c_stateful_fake_sem", self.FAKE_PY)
        assert spec and spec.loader
        module = importlib.util.module_from_spec(spec)
        sys.modules["phase1c_stateful_fake_sem"] = module
        spec.loader.exec_module(module)
        return module

    def test_ask_and_stream_bodies_use_question(self) -> None:
        denial = load_denial()
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = self._seed_creds(seed, probes)
        ask_body = denial._owner_body("ask", seed, credentials=creds, params={})
        stream_body = denial._owner_body("askStream", seed, credentials=creds, params={})
        assert ask_body is not None and stream_body is not None
        self.assertIn("question", ask_body)
        self.assertNotIn("query", ask_body)
        self.assertIn("question", stream_body)

    def test_append_chat_turn_body_matches_production(self) -> None:
        denial = load_denial()
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = self._seed_creds(seed, probes)
        body = denial._owner_body(
            "appendChatTurn",
            seed,
            credentials=creds,
            params={"sessionId": seed.beta_chat_session_id},
        )
        assert body is not None
        for key in ("question", "answer", "answerMode"):
            self.assertIn(key, body)

    def test_current_org_ops_include_authenticated_foreign_isolation(self) -> None:
        denial = load_denial()
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = self._seed_creds(seed, probes)
        entry = next(e for e in denial.build_http_sse_denial_mapping() if e.operation_id == "authMe")
        specs = denial.build_row_denial_specs(entry, seed=seed, credentials=creds)
        self.assertIn("authenticated_foreign_isolation", {s.scenario for s in specs})

    def test_quota_probe_fails_closed_without_docker(self) -> None:
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
        with mock.patch("phase1c_deployed_probes.shutil.which", return_value=None):
            with self.assertRaises(RuntimeError):
                runner.run_quota_recovery_probe()

    def test_qdrant_probe_uses_distinct_leakage_metric(self) -> None:
        probes = load_probes()
        seed = self._seed_fixture(probes)
        creds = self._seed_creds(seed, probes)

        class QdrantShim(probes.DeployedProbeShims):
            def compose(self, args, **kwargs):  # type: ignore[override]
                if args[:2] == ["stop", "qdrant"]:
                    return probes.CommandOutcome(0, "", "")
                if args[:2] == ["start", "qdrant"]:
                    return probes.CommandOutcome(0, "", "")
                if args[:2] == ["ps", "-q"]:
                    return probes.CommandOutcome(0, "qdrant\n", "")
                return probes.CommandOutcome(1, "", "")

            def psql(self, sql, **kwargs):  # type: ignore[override]
                import hashlib

                digest = hashlib.sha256(seed.challenge.encode("utf-8")).hexdigest()
                return probes.CommandOutcome(0, digest, "")

            def http_request(self, **kwargs):  # type: ignore[override]
                path = kwargs.get("path") or ""
                degraded = json.dumps(
                    {
                        "items": [],
                        "warnings": ["Vector leg unavailable; continuing with FTS-only retrieval."],
                        "requestId": "11111111-1111-1111-1111-111111111111",
                    }
                )
                if path.endswith("/ask"):
                    return probes.HttpResponse(status=200, body=degraded, headers={})
                if path.endswith("/search"):
                    return probes.HttpResponse(status=200, body=degraded, headers={})
                return probes.HttpResponse(status=503, body="{}", headers={})

        runner = probes.DeployedProbeRunner(
            api_base="http://fake",
            seed=seed,
            credentials=creds,
            shims=QdrantShim(),
            git_sha_full=seed.source_revision["commit"],
        )
        with mock.patch("phase1c_deployed_probes.shutil.which", return_value="/usr/bin/docker"):
            result = runner.run_qdrant_fail_closed_probe()
        self.assertIn("qdrant_degraded_leakage_count", result.metrics)
        self.assertEqual(result.probe.get("searchStatus"), 200)
        self.assertNotIn("cross_tenant_leakage_count", result.metrics)

    def test_quota_probe_orders_audit_log_by_created_at(self) -> None:
        text = (ROOT / "bench/markhand_web/scripts/phase1c_deployed_probes.py").read_text(encoding="utf-8")
        self.assertIn("ORDER BY created_at DESC", text)
        self.assertNotIn("ORDER BY occurred_at DESC", text)

    def test_seed_fixture_parses_duplicate_collection_ids(self) -> None:
        probes = load_probes()
        seed = probes.parse_seed_artifact(
            complete_seed_raw(probes),
            expected_challenge="phase1c-challenge-abc",
        )
        self.assertEqual(seed.alpha_duplicate_collection_id, "12121212-1212-1212-1212-121212121211")
        self.assertEqual(seed.beta_duplicate_collection_id, "23232323-2323-2323-2323-232323232322")


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
