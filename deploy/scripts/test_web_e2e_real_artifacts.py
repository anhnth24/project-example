#!/usr/bin/env python3
"""Focused hermetic tests for web_e2e_real_artifacts.py write/validate."""

from __future__ import annotations

import hashlib
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "web_e2e_real_artifacts.py"
REPO_ROOT = SCRIPT.resolve().parents[2]

# Canonical P2-20 inventory — must match
# `MARKHAND_E2E_REAL=1 pnpm --dir web exec playwright test --list` titles.
REQUIRED_SCENARIO_TITLES = (
    "reindex on an indexed document shows the enqueue success notice",
    "fixture failed document shows the failed badge and retry enqueues reindex",
    "delete with confirm removes the document row after refetch",
    "viewer reindex is denied with a real HTTP 403 and the document remains",
    "reindex under the lowered route limit returns a real 429 with retry-after copy",
    "login with runtime credentials shows the in-app shell",
    "logout returns to /login without the library rail",
    "anonymous deep-link to the run collection preserves ?next= through login",
    "a one-shot invalid bearer on GET /auth/me recovers via real refresh without /login bounce",
    "navigating to the run collection shows the upload panel",
    "uploading a unique text document indexes and previews markdown",
    "downloading Markdown issues a capability, redeems it, and does not log the token",
    "uploading a file against the real backend reaches indexed, and its preview renders",
    "a delayed POST /uploads shows upload progress then reaches indexed preview",
    "a real oversized upload returns 413 and the too-large alert without an indexed row",
)


def _sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _scenario_entry(
    title: str,
    *,
    status: str = "passed",
    duration: int = 120,
) -> dict:
    return {
        "title": title,
        "ok": status == "passed",
        "tests": [
            {
                "projectName": "real",
                "results": [
                    {
                        "status": status,
                        "duration": duration,
                        "errors": [{"message": "SECRET_SHOULD_NOT_LEAK"}],
                        "stdout": [{"text": "document body CANARY_DOC"}],
                        "stderr": [],
                    }
                ],
                "status": "expected" if status == "passed" else "unexpected",
            }
        ],
    }


def _playwright_results(
    *,
    titles: tuple[str, ...] | None = None,
    status: str = "passed",
    duration: int = 120,
    skipped: int = 0,
    title: str | None = None,
) -> dict:
    if titles is None:
        if title is not None:
            titles = (title,)
        else:
            titles = REQUIRED_SCENARIO_TITLES
    specs = [_scenario_entry(item, status=status, duration=duration) for item in titles]
    expected = len(titles) if status == "passed" else 0
    unexpected = 0 if status == "passed" else len(titles)
    return {
        "suites": [
            {
                "title": "e2e-real",
                "file": "e2e-real",
                "specs": specs,
                "suites": [],
            }
        ],
        "errors": [{"message": "raw error must not stage"}],
        "stats": {
            "duration": duration * max(len(titles), 1),
            "expected": expected,
            "skipped": skipped,
            "unexpected": unexpected,
            "flaky": 0,
        },
    }


def _complete_scenarios(
    *,
    outcome: str = "passed",
    duration_ms: int = 120,
    omit: str | None = None,
    duplicate: str | None = None,
    mutate_title: tuple[str, str] | None = None,
) -> list[dict]:
    scenarios: list[dict] = []
    for title in REQUIRED_SCENARIO_TITLES:
        if omit is not None and title == omit:
            continue
        use_title = title
        if mutate_title is not None and title == mutate_title[0]:
            use_title = mutate_title[1]
        scenarios.append(
            {"title": use_title, "outcome": outcome, "durationMs": duration_ms}
        )
    if duplicate is not None:
        scenarios.append(
            {"title": duplicate, "outcome": outcome, "durationMs": duration_ms}
        )
    return scenarios


def _valid_manifest_payload(**overrides: object) -> dict:
    payload: dict = {
        "schemaVersion": 1,
        "runId": "e2e-abcdef012345-1",
        "git": {
            "sha": "e4f289efb5467f877c5901515c646f5dc6e253ba",
            "ref": "cursor/p2-20-real-e2e-foundation-e9d6",
        },
        "toolVersions": {
            "node": "v20.19.0",
            "pnpm": "10.33.3",
            "playwright": "Version 1.55.0",
        },
        "fixtureChecksum": "a" * 64,
        "scenarios": _complete_scenarios(),
        "skippedCount": 0,
        "teardown": {"result": "ok"},
        "artifactChecksums": {},
    }
    payload.update(overrides)
    return payload


def _fixture_manifest(run_id: str = "e2e-abcdef012345-1") -> dict:
    return {
        "runId": run_id,
        "orgId": "11111111-1111-1111-1111-111111111111",
        "adminUserId": "22222222-2222-2222-2222-222222222201",
        "viewerUserId": "22222222-2222-2222-2222-222222222202",
        "collectionId": "33333333-3333-3333-3333-333333333301",
        "collectionName": f"E2E Library {run_id}",
        "failedDocumentId": "44444444-4444-4444-4444-444444444401",
        "failedVersionId": "44444444-4444-4444-4444-444444444402",
        "objectIds": ["55555555-5555-5555-5555-555555555501"],
        "vectorPointIds": ["66666666-6666-6666-6666-666666666601"],
        "checksum": "a" * 64,
    }


class ArtifactHelperTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tempdir = Path(tempfile.mkdtemp(prefix="web-e2e-real-artifacts-"))
        self.artifact_dir = self.tempdir / "artifacts"
        self.artifact_dir.mkdir()
        self.results_path = self.tempdir / "playwright-results.json"
        self.fixture_path = self.tempdir / "fixture.json"
        self.manifest_path = self.artifact_dir / "manifest.json"
        self.fixture_path.write_text(
            json.dumps(_fixture_manifest()), encoding="utf-8"
        )
        self.results_path.write_text(
            json.dumps(_playwright_results()), encoding="utf-8"
        )

    def tearDown(self) -> None:
        for child in sorted(self.tempdir.rglob("*"), reverse=True):
            if child.is_file() or child.is_symlink():
                child.unlink(missing_ok=True)
            elif child.is_dir():
                child.rmdir()
        self.tempdir.rmdir()

    def _run(self, *args: str, env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
        merged = os.environ.copy()
        if env:
            merged.update(env)
        return subprocess.run(
            [sys.executable, str(SCRIPT), *args],
            cwd=str(REPO_ROOT / "deploy" / "scripts"),
            env=merged,
            text=True,
            capture_output=True,
            check=False,
        )

    def _write_manifest_file(self, payload: dict) -> None:
        self.manifest_path.write_text(
            json.dumps(payload, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )

    def test_write_extracts_only_safe_scenario_fields(self) -> None:
        result = self._run(
            "write",
            "--results",
            str(self.results_path),
            "--fixture",
            str(self.fixture_path),
            "--out",
            str(self.manifest_path),
            "--teardown",
            "ok",
        )
        self.assertEqual(result.returncode, 0, msg=result.stderr)
        manifest = json.loads(self.manifest_path.read_text(encoding="utf-8"))
        text = json.dumps(manifest)
        self.assertNotIn("SECRET_SHOULD_NOT_LEAK", text)
        self.assertNotIn("CANARY_DOC", text)
        self.assertNotIn("raw error must not stage", text)
        self.assertEqual(manifest["skippedCount"], 0)
        self.assertEqual(manifest["fixtureChecksum"], "a" * 64)
        self.assertEqual(manifest["teardown"]["result"], "ok")
        self.assertEqual(len(manifest["scenarios"]), len(REQUIRED_SCENARIO_TITLES))
        titles = [item["title"] for item in manifest["scenarios"]]
        self.assertEqual(sorted(titles), sorted(REQUIRED_SCENARIO_TITLES))
        for scenario in manifest["scenarios"]:
            self.assertEqual(scenario["outcome"], "passed")
            self.assertEqual(scenario["durationMs"], 120)
            self.assertEqual(set(scenario.keys()), {"title", "outcome", "durationMs"})
        self.assertIn("git", manifest)
        self.assertIn("sha", manifest["git"])
        self.assertIn("ref", manifest["git"])
        self.assertIn("toolVersions", manifest)
        self.assertEqual(manifest["artifactChecksums"], {})
        self.assertIn("artifactChecksums", manifest)

    def test_validate_rejects_skipped_required_scenario(self) -> None:
        self.results_path.write_text(
            json.dumps(_playwright_results(status="skipped", skipped=1)),
            encoding="utf-8",
        )
        write = self._run(
            "write",
            "--results",
            str(self.results_path),
            "--fixture",
            str(self.fixture_path),
            "--out",
            str(self.manifest_path),
            "--teardown",
            "ok",
        )
        self.assertEqual(write.returncode, 0, msg=write.stderr)
        validate = self._run(
            "validate",
            "--manifest",
            str(self.manifest_path),
            "--artifact-dir",
            str(self.artifact_dir),
        )
        self.assertNotEqual(validate.returncode, 0)

    def test_validate_rejects_failed_teardown(self) -> None:
        write = self._run(
            "write",
            "--results",
            str(self.results_path),
            "--fixture",
            str(self.fixture_path),
            "--out",
            str(self.manifest_path),
            "--teardown",
            "failed",
        )
        self.assertEqual(write.returncode, 0, msg=write.stderr)
        validate = self._run(
            "validate",
            "--manifest",
            str(self.manifest_path),
            "--artifact-dir",
            str(self.artifact_dir),
        )
        self.assertNotEqual(validate.returncode, 0)

    def test_validate_rejects_checksum_mismatch(self) -> None:
        # With manifest-only allowlist there are no companion digests; mutate the
        # on-disk companion inventory map to claim a digest for a missing path.
        self._write_manifest_file(
            _valid_manifest_payload(
                artifactChecksums={"ghost-companion.txt": "0" * 64},
            )
        )
        validate = self._run(
            "validate",
            "--manifest",
            str(self.manifest_path),
            "--artifact-dir",
            str(self.artifact_dir),
        )
        self.assertNotEqual(validate.returncode, 0)

    def test_validate_rejects_secret_canary_match(self) -> None:
        payload = _valid_manifest_payload(runId="leak-token-CANARY_SECRET")
        self._write_manifest_file(payload)
        validate = self._run(
            "validate",
            "--manifest",
            str(self.manifest_path),
            "--artifact-dir",
            str(self.artifact_dir),
            env={"WEB_E2E_REAL_SECRET_CANARIES": "CANARY_SECRET"},
        )
        self.assertNotEqual(validate.returncode, 0)
        self.assertIn("canary", validate.stderr.lower())

    def test_validate_rejects_content_canary_match(self) -> None:
        payload = _valid_manifest_payload(runId="indexed-preview-CANARY_DOC_BODY")
        self._write_manifest_file(payload)
        validate = self._run(
            "validate",
            "--manifest",
            str(self.manifest_path),
            "--artifact-dir",
            str(self.artifact_dir),
            env={"WEB_E2E_REAL_CONTENT_CANARIES": "CANARY_DOC_BODY"},
        )
        self.assertNotEqual(validate.returncode, 0)
        self.assertIn("canary", validate.stderr.lower())

    def test_validate_rejects_missing_scenario(self) -> None:
        omitted = REQUIRED_SCENARIO_TITLES[0]
        self._write_manifest_file(
            _valid_manifest_payload(scenarios=_complete_scenarios(omit=omitted))
        )
        validate = self._run(
            "validate",
            "--manifest",
            str(self.manifest_path),
            "--artifact-dir",
            str(self.artifact_dir),
        )
        self.assertNotEqual(validate.returncode, 0)
        self.assertIn("scenario", validate.stderr.lower())

    def test_validate_rejects_empty_scenario_list(self) -> None:
        self._write_manifest_file(_valid_manifest_payload(scenarios=[]))
        validate = self._run(
            "validate",
            "--manifest",
            str(self.manifest_path),
            "--artifact-dir",
            str(self.artifact_dir),
        )
        self.assertNotEqual(validate.returncode, 0)
        self.assertIn("scenario", validate.stderr.lower())

    def test_validate_rejects_duplicate_scenario_identity(self) -> None:
        dup = REQUIRED_SCENARIO_TITLES[2]
        self._write_manifest_file(
            _valid_manifest_payload(scenarios=_complete_scenarios(duplicate=dup))
        )
        validate = self._run(
            "validate",
            "--manifest",
            str(self.manifest_path),
            "--artifact-dir",
            str(self.artifact_dir),
        )
        self.assertNotEqual(validate.returncode, 0)
        self.assertIn("duplicate", validate.stderr.lower())

    def test_validate_rejects_mutated_scenario_identity(self) -> None:
        original = REQUIRED_SCENARIO_TITLES[3]
        self._write_manifest_file(
            _valid_manifest_payload(
                scenarios=_complete_scenarios(
                    mutate_title=(original, "renamed scenario that is not required"),
                )
            )
        )
        validate = self._run(
            "validate",
            "--manifest",
            str(self.manifest_path),
            "--artifact-dir",
            str(self.artifact_dir),
        )
        self.assertNotEqual(validate.returncode, 0)
        self.assertIn("scenario", validate.stderr.lower())

    def test_validate_rejects_failed_outcome(self) -> None:
        self._write_manifest_file(
            _valid_manifest_payload(scenarios=_complete_scenarios(outcome="failed"))
        )
        validate = self._run(
            "validate",
            "--manifest",
            str(self.manifest_path),
            "--artifact-dir",
            str(self.artifact_dir),
        )
        self.assertNotEqual(validate.returncode, 0)
        self.assertIn("outcome", validate.stderr.lower())

    def test_validate_rejects_timed_out_and_flaky_outcomes(self) -> None:
        for outcome in ("timedOut", "flaky", "skipped", "interrupted"):
            with self.subTest(outcome=outcome):
                scenarios = _complete_scenarios()
                scenarios[1]["outcome"] = outcome
                self._write_manifest_file(
                    _valid_manifest_payload(scenarios=scenarios)
                )
                validate = self._run(
                    "validate",
                    "--manifest",
                    str(self.manifest_path),
                    "--artifact-dir",
                    str(self.artifact_dir),
                )
                self.assertNotEqual(validate.returncode, 0)
                self.assertIn("outcome", validate.stderr.lower())

    def test_validate_rejects_malformed_git_metadata(self) -> None:
        cases = (
            {"sha": "unknown", "ref": "cursor/p2-20-real-e2e-foundation-e9d6"},
            {"sha": "e4f289efb5467f877c5901515c646f5dc6e253ba", "ref": "unknown"},
            {"sha": "", "ref": "cursor/p2-20-real-e2e-foundation-e9d6"},
            {"sha": "not-a-sha", "ref": "cursor/p2-20-real-e2e-foundation-e9d6"},
            {"sha": "e4f289efb5467f877c5901515c646f5dc6e253ba", "ref": ""},
        )
        for git in cases:
            with self.subTest(git=git):
                self._write_manifest_file(_valid_manifest_payload(git=git))
                validate = self._run(
                    "validate",
                    "--manifest",
                    str(self.manifest_path),
                    "--artifact-dir",
                    str(self.artifact_dir),
                )
                self.assertNotEqual(validate.returncode, 0)
                self.assertIn("git", validate.stderr.lower())

    def test_validate_rejects_invalid_fixture_checksum(self) -> None:
        for checksum in ("", "not-hex", "a" * 63, "a" * 65, "A" * 64):
            with self.subTest(checksum=checksum):
                self._write_manifest_file(
                    _valid_manifest_payload(fixtureChecksum=checksum)
                )
                validate = self._run(
                    "validate",
                    "--manifest",
                    str(self.manifest_path),
                    "--artifact-dir",
                    str(self.artifact_dir),
                )
                self.assertNotEqual(validate.returncode, 0)
                self.assertIn("checksum", validate.stderr.lower())

    def test_validate_rejects_empty_tool_version_values(self) -> None:
        self._write_manifest_file(
            _valid_manifest_payload(
                toolVersions={"node": "", "pnpm": "10.33.3", "playwright": "Version 1.55.0"}
            )
        )
        validate = self._run(
            "validate",
            "--manifest",
            str(self.manifest_path),
            "--artifact-dir",
            str(self.artifact_dir),
        )
        self.assertNotEqual(validate.returncode, 0)
        self.assertIn("tool", validate.stderr.lower())

    def test_validate_rejects_unallowlisted_companion(self) -> None:
        self._write_manifest_file(_valid_manifest_payload())
        planted = self.artifact_dir / "unlisted-customer-document.txt"
        planted.write_text("customer document body\n", encoding="utf-8")
        validate = self._run(
            "validate",
            "--manifest",
            str(self.manifest_path),
            "--artifact-dir",
            str(self.artifact_dir),
        )
        self.assertNotEqual(validate.returncode, 0)
        self.assertRegex(validate.stderr.lower(), r"allowlist|companion|unallowlisted")

    def test_write_does_not_retain_unallowlisted_preexisting_companions(self) -> None:
        planted = self.artifact_dir / "unlisted-customer-document.txt"
        planted.write_text("customer document body\n", encoding="utf-8")
        write = self._run(
            "write",
            "--results",
            str(self.results_path),
            "--fixture",
            str(self.fixture_path),
            "--out",
            str(self.manifest_path),
            "--teardown",
            "ok",
        )
        self.assertEqual(write.returncode, 0, msg=write.stderr)
        manifest = json.loads(self.manifest_path.read_text(encoding="utf-8"))
        self.assertNotIn("unlisted-customer-document.txt", manifest["artifactChecksums"])
        self.assertEqual(manifest["artifactChecksums"], {})
        validate = self._run(
            "validate",
            "--manifest",
            str(self.manifest_path),
            "--artifact-dir",
            str(self.artifact_dir),
        )
        self.assertNotEqual(validate.returncode, 0)

    def test_validate_rejects_nonzero_skipped_count(self) -> None:
        self._write_manifest_file(_valid_manifest_payload(skippedCount=2))
        validate = self._run(
            "validate",
            "--manifest",
            str(self.manifest_path),
            "--artifact-dir",
            str(self.artifact_dir),
        )
        self.assertNotEqual(validate.returncode, 0)
        self.assertIn("skipped", validate.stderr.lower())

    def test_validate_accepts_sanitized_manifest(self) -> None:
        write = self._run(
            "write",
            "--results",
            str(self.results_path),
            "--fixture",
            str(self.fixture_path),
            "--out",
            str(self.manifest_path),
            "--teardown",
            "ok",
        )
        self.assertEqual(write.returncode, 0, msg=write.stderr)
        # Writer records live git/tool metadata; keep those and only assert validate.
        manifest = json.loads(self.manifest_path.read_text(encoding="utf-8"))
        self.assertEqual(len(manifest["scenarios"]), 15)
        self.assertEqual(manifest["artifactChecksums"], {})
        validate = self._run(
            "validate",
            "--manifest",
            str(self.manifest_path),
            "--artifact-dir",
            str(self.artifact_dir),
            env={
                "WEB_E2E_REAL_SECRET_CANARIES": "CANARY_SECRET",
                "WEB_E2E_REAL_CONTENT_CANARIES": "CANARY_DOC_BODY",
            },
        )
        self.assertEqual(validate.returncode, 0, msg=validate.stderr)

    def test_validate_accepts_complete_handwritten_inventory(self) -> None:
        self._write_manifest_file(_valid_manifest_payload())
        validate = self._run(
            "validate",
            "--manifest",
            str(self.manifest_path),
            "--artifact-dir",
            str(self.artifact_dir),
        )
        self.assertEqual(validate.returncode, 0, msg=validate.stderr)

    def test_validate_rejects_missing_required_fields(self) -> None:
        self.manifest_path.write_text(json.dumps({"runId": "x"}), encoding="utf-8")
        validate = self._run(
            "validate",
            "--manifest",
            str(self.manifest_path),
            "--artifact-dir",
            str(self.artifact_dir),
        )
        self.assertNotEqual(validate.returncode, 0)

    def test_write_refuses_missing_results(self) -> None:
        missing = self.tempdir / "missing-results.json"
        result = self._run(
            "write",
            "--results",
            str(missing),
            "--fixture",
            str(self.fixture_path),
            "--out",
            str(self.manifest_path),
            "--teardown",
            "ok",
        )
        self.assertNotEqual(result.returncode, 0)


def main() -> int:
    loader = unittest.TestLoader()
    suite = loader.loadTestsFromModule(sys.modules[__name__])
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    raise SystemExit(main())
