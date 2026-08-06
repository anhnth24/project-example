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


def _sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _playwright_results(
    *,
    title: str = "login succeeds",
    status: str = "passed",
    duration: int = 120,
    skipped: int = 0,
) -> dict:
    return {
        "suites": [
            {
                "title": "auth.spec.ts",
                "file": "e2e-real/auth.spec.ts",
                "specs": [
                    {
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
                ],
                "suites": [],
            }
        ],
        "errors": [{"message": "raw error must not stage"}],
        "stats": {
            "duration": duration,
            "expected": 0 if status != "passed" else 1,
            "skipped": skipped,
            "unexpected": 0 if status == "passed" else 1,
            "flaky": 0,
        },
    }


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
        self.assertEqual(len(manifest["scenarios"]), 1)
        scenario = manifest["scenarios"][0]
        self.assertEqual(scenario["title"], "login succeeds")
        self.assertEqual(scenario["outcome"], "passed")
        self.assertEqual(scenario["durationMs"], 120)
        self.assertIn("git", manifest)
        self.assertIn("sha", manifest["git"])
        self.assertIn("ref", manifest["git"])
        self.assertIn("toolVersions", manifest)
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
        companion = self.artifact_dir / "summary.txt"
        companion.write_text("ok\n", encoding="utf-8")
        manifest = json.loads(self.manifest_path.read_text(encoding="utf-8"))
        manifest["artifactChecksums"]["summary.txt"] = "0" * 64
        self.manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
        validate = self._run(
            "validate",
            "--manifest",
            str(self.manifest_path),
            "--artifact-dir",
            str(self.artifact_dir),
        )
        self.assertNotEqual(validate.returncode, 0)

    def test_validate_rejects_secret_canary_match(self) -> None:
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
        planted = self.artifact_dir / "note.txt"
        planted.write_text("leak-token-CANARY_SECRET\n", encoding="utf-8")
        manifest = json.loads(self.manifest_path.read_text(encoding="utf-8"))
        manifest["artifactChecksums"]["note.txt"] = _sha256(planted)
        self.manifest_path.write_text(json.dumps(manifest, indent=2), encoding="utf-8")
        validate = self._run(
            "validate",
            "--manifest",
            str(self.manifest_path),
            "--artifact-dir",
            str(self.artifact_dir),
            env={"WEB_E2E_REAL_SECRET_CANARIES": "CANARY_SECRET"},
        )
        self.assertNotEqual(validate.returncode, 0)

    def test_validate_rejects_content_canary_match(self) -> None:
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
        planted = self.artifact_dir / "preview.txt"
        planted.write_text("indexed preview CANARY_DOC_BODY retained\n", encoding="utf-8")
        manifest = json.loads(self.manifest_path.read_text(encoding="utf-8"))
        manifest["artifactChecksums"]["preview.txt"] = _sha256(planted)
        self.manifest_path.write_text(json.dumps(manifest, indent=2), encoding="utf-8")
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
        manifest["scenarios"] = []
        self.manifest_path.write_text(json.dumps(manifest, indent=2), encoding="utf-8")
        validate = self._run(
            "validate",
            "--manifest",
            str(self.manifest_path),
            "--artifact-dir",
            str(self.artifact_dir),
        )
        self.assertNotEqual(validate.returncode, 0)
        self.assertIn("scenarios", validate.stderr.lower())

    def test_validate_rejects_nonzero_skipped_count(self) -> None:
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
        manifest["skippedCount"] = 2
        self.manifest_path.write_text(json.dumps(manifest, indent=2), encoding="utf-8")
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
        companion = self.artifact_dir / "summary.txt"
        companion.write_text("sanitized summary\n", encoding="utf-8")
        rewrite = self._run(
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
        self.assertEqual(rewrite.returncode, 0, msg=rewrite.stderr)
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
