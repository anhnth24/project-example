#!/usr/bin/env python3
"""Validate P1B-O04 vertical-slice/security harness (hermetic; no Docker required).

Checks:
- suite + fixture manifests present and schema-shaped
- fixture checksum integrity + adversarial fixtures present
- redaction / confirm-gate unit tests
- deploy script / seed script syntax (bash -n)
- evidence schema + forbidden fields
- regenerates hermetic evidence report with claimsLiveVerticalSlice=false

Does NOT claim a live vertical slice passed. Invoking the live script without
Docker/prereqs must fail (verified by static inspection of fail-closed gates).
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
E2E = ROOT / "crates" / "server" / "tests" / "e2e"
SUITE = E2E / "manifest.json"
FIXTURE_MANIFEST = E2E / "fixtures" / "manifest.json"
FIXTURE_GEN = E2E / "fixtures" / "generate.py"
EVIDENCE_SCHEMA = E2E / "schema" / "evidence.schema.json"
SUITE_SCHEMA = E2E / "schema" / "suite-manifest.schema.json"
POC_E2E_MANIFEST = ROOT / "deploy" / "poc" / "e2e-manifest.json"
LIVE_SH = ROOT / "deploy" / "scripts" / "poc-e2e-o04.sh"
SEED_SH = ROOT / "deploy" / "scripts" / "seed-poc-e2e.sh"
COMPOSE = ROOT / "deploy" / "compose.poc.yml"
REPORT_MD = ROOT / "bench" / "markhand_web" / "reports" / "p1b-o04-vertical-slice.md"
REPORT_JSON = ROOT / "bench" / "markhand_web" / "reports" / "p1b-o04-vertical-slice.json"
IMAGES_LOCK = ROOT / "deploy" / "poc" / "images.lock.json"

sys.path.insert(0, str(E2E))
from harness.confirm import DEFAULT_CONFIRM, validate_live_gates  # noqa: E402
from harness.intake import ProductionIntakeNotWired, extract_production_intake  # noqa: E402
from harness.redaction import assert_no_forbidden_evidence, redact_value, scrub_text  # noqa: E402
from harness.runner import load_suite_manifest, run_hermetic_blocked_report  # noqa: E402


class HarnessError(RuntimeError):
    pass


def load_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise HarnessError(f"{path}: {error}") from error


def require_file(path: Path) -> None:
    if not path.is_file():
        raise HarnessError(f"missing required file: {path.relative_to(ROOT)}")


def validate_suite_shape(suite: dict[str, Any]) -> None:
    for key in (
        "version",
        "issue",
        "apiBasePath",
        "composeServices",
        "confirmPhrase",
        "formats",
        "security",
        "fault",
        "evidence",
    ):
        if key not in suite:
            raise HarnessError(f"suite manifest missing {key}")
    if suite["issue"] != "P1B-O04":
        raise HarnessError("suite issue must be P1B-O04")
    if suite["apiBasePath"] != "/api/v1":
        raise HarnessError("apiBasePath must be /api/v1")
    if suite["confirmPhrase"] != DEFAULT_CONFIRM:
        raise HarnessError("confirmPhrase drift vs harness.confirm.DEFAULT_CONFIRM")
    services = suite["composeServices"]
    for required in ("api", "postgres", "minio", "qdrant", "workerConvert", "workerIndex"):
        if required not in services:
            raise HarnessError(f"composeServices missing {required}")
    # Service names must exist in compose.poc.yml
    compose_text = COMPOSE.read_text(encoding="utf-8")
    for name in services.values():
        if f"  {name}:" not in compose_text:
            raise HarnessError(f"compose.poc.yml missing service {name}")
    if len(suite["formats"]) < 8:
        raise HarnessError("formats matrix too small")
    if len(suite["security"]) < 10:
        raise HarnessError("security matrix too small")
    if len(suite["fault"]) < 3:
        raise HarnessError("fault matrix too small")
    audio = [f for f in suite["formats"] if f["id"] == "fmt-audio"]
    if not audio or audio[0].get("requirement") != "optional_model":
        raise HarnessError("audio format must be classified optional_model")
    for fmt in suite["formats"]:
        steps = fmt.get("steps") or []
        if "bridge" in steps:
            raise HarnessError(f"{fmt['id']}: bridge step is forbidden")
        if "require_production_intake_ids" not in steps:
            raise HarnessError(f"{fmt['id']}: missing require_production_intake_ids step")
    # Fail-honest: no intake bridge artifacts.
    forbidden = [
        E2E / "harness" / "bridge.py",
        E2E / "sql" / "bridge_upload.sql",
    ]
    for path in forbidden:
        if path.exists():
            raise HarnessError(f"forbidden intake bridge artifact present: {path.relative_to(ROOT)}")


def validate_fixtures() -> None:
    proc = subprocess.run(
        [sys.executable, str(FIXTURE_GEN), "--check"],
        cwd=ROOT,
        check=False,
        text=True,
        capture_output=True,
    )
    if proc.returncode != 0:
        raise HarnessError(f"fixture integrity failed:\n{proc.stderr or proc.stdout}")
    data = load_json(FIXTURE_MANIFEST)
    ids = {f["id"] for f in data["fixtures"]}
    required = {
        "e2e-vi-txt",
        "e2e-vi-html",
        "e2e-vi-csv",
        "e2e-vi-pdf",
        "e2e-vi-docx",
        "e2e-vi-pptx",
        "e2e-vi-xlsx",
        "e2e-vi-png",
        "e2e-vi-wav",
        "e2e-adv-spoof-pdf",
        "e2e-adv-prompt-html",
        "e2e-adv-traversal",
        "e2e-adv-zip-bomb",
        "e2e-adv-malformed-docx",
    }
    missing = sorted(required - ids)
    if missing:
        raise HarnessError(f"fixture manifest missing ids: {missing}")
    # Adversarial zip bomb should be small on disk but expand larger.
    bomb = next(f for f in data["fixtures"] if f["id"] == "e2e-adv-zip-bomb")
    bomb_path = E2E / "fixtures" / bomb["path"]
    if bomb_path.stat().st_size > 64 * 1024:
        raise HarnessError("zip bomb fixture unexpectedly large on disk")
    # OCR PNG must be a real rendered token image, not a tiny blank structural PNG.
    png = next(f for f in data["fixtures"] if f["id"] == "e2e-vi-png")
    png_path = E2E / "fixtures" / png["path"]
    png_bytes = png_path.read_bytes()
    if len(png_bytes) < 400:
        raise HarnessError("OCR PNG too small — blank structural PNG is not allowed")
    if not png_bytes.startswith(b"\x89PNG\r\n\x1a\n"):
        raise HarnessError("OCR fixture is not a PNG")
    # No secrets in fixtures.
    for fixture in data["fixtures"]:
        content = (E2E / "fixtures" / fixture["path"]).read_bytes()
        if b"BEGIN PRIVATE KEY" in content or b"postgres://" in content:
            raise HarnessError(f"secret canary in fixture {fixture['id']}")


def validate_scripts() -> None:
    require_file(LIVE_SH)
    require_file(SEED_SH)
    if not os.access(LIVE_SH, os.X_OK) or not os.access(SEED_SH, os.X_OK):
        # Ensure executable bit in git; still allow bash -n.
        pass
    for script in (LIVE_SH, SEED_SH):
        proc = subprocess.run(
            ["bash", "-n", str(script)],
            check=False,
            text=True,
            capture_output=True,
        )
        if proc.returncode != 0:
            raise HarnessError(f"bash -n failed for {script.name}: {proc.stderr}")
    live_text = LIVE_SH.read_text(encoding="utf-8")
    for needle in (
        "MARKHAND_E2E_CONFIRM",
        "MARKHAND_E2E_STACK_TAG",
        "poc-up.sh",
        "seed-poc-e2e.sh",
        "run_live.py",
        "die ",
    ):
        if needle not in live_text:
            raise HarnessError(f"poc-e2e-o04.sh missing fail-closed marker: {needle}")
    if 'die "Docker engine not available"' not in live_text:
        raise HarnessError("live script must die when Docker unavailable")
    confirm_at = live_text.find("MARKHAND_E2E_CONFIRM")
    docker_at = live_text.find("require_cmd docker")
    if confirm_at < 0 or docker_at < 0 or confirm_at > docker_at:
        raise HarnessError("confirm gate must run before require_cmd docker")


def validate_poc_manifest() -> None:
    data = load_json(POC_E2E_MANIFEST)
    if data.get("issue") != "P1B-O04":
        raise HarnessError("deploy/poc/e2e-manifest.json issue mismatch")
    if data.get("confirmPhrase") != DEFAULT_CONFIRM:
        raise HarnessError("poc e2e-manifest confirmPhrase drift")
    require_file(IMAGES_LOCK)


def validate_schemas() -> None:
    for path in (EVIDENCE_SCHEMA, SUITE_SCHEMA):
        schema = load_json(path)
        if schema.get("type") != "object":
            raise HarnessError(f"{path.name}: expected object schema")


class O04SelfTests(unittest.TestCase):
    def test_confirm_rejects_human_stack(self) -> None:
        result = validate_live_gates(
            environ={
                "MARKHAND_E2E_CONFIRM": DEFAULT_CONFIRM,
                "MARKHAND_COMPOSE_PROJECT": "markhand-poc",
                "MARKHAND_POSTGRES_DB": "markhand",
                "MARKHAND_MINIO_BUCKET": "markhand-documents",
                "MARKHAND_E2E_STACK_TAG": "test",
            }
        )
        self.assertFalse(result.ok)
        self.assertTrue(any("COMPOSE_PROJECT" in e for e in result.errors))

    def test_confirm_accepts_tagged_test_stack(self) -> None:
        result = validate_live_gates(
            environ={
                "MARKHAND_E2E_CONFIRM": DEFAULT_CONFIRM,
                "MARKHAND_COMPOSE_PROJECT": "markhand-e2e-ci",
                "MARKHAND_POSTGRES_DB": "markhand_e2e",
                "MARKHAND_MINIO_BUCKET": "markhand-e2e-docs",
                "MARKHAND_E2E_STACK_TAG": "test",
            }
        )
        self.assertTrue(result.ok, result.errors)

    def test_confirm_wrong_phrase(self) -> None:
        result = validate_live_gates(
            environ={
                "MARKHAND_E2E_CONFIRM": "yes",
                "MARKHAND_COMPOSE_PROJECT": "markhand-e2e",
                "MARKHAND_POSTGRES_DB": "markhand_e2e",
                "MARKHAND_MINIO_BUCKET": "markhand-e2e",
                "MARKHAND_E2E_STACK_TAG": "test",
            }
        )
        self.assertFalse(result.ok)

    def test_redaction_strips_tokens_and_keys(self) -> None:
        dirty = (
            'Bearer abc.def.ghi "accessToken":"tok123" '
            "quarantine/" + ("a" * 64) + "/" + ("b" * 32) + " "
            "postgres://user:pass@host/db"
        )
        clean = scrub_text(dirty)
        self.assertNotIn("Bearer abc", clean)
        self.assertNotIn("tok123", clean)
        self.assertNotIn("postgres://", clean)
        self.assertNotIn("quarantine/", clean)
        payload = redact_value(
            {
                "accessToken": "secret",
                "orgId": "11111111-1111-1111-1111-111111111111",
                "ok": True,
            }
        )
        self.assertEqual(payload["accessToken"], "[REDACTED]")
        self.assertEqual(payload["orgId"], "[REDACTED]")
        leaks = assert_no_forbidden_evidence(json.dumps(payload))
        self.assertEqual(leaks, [])

    def test_intake_requires_production_ids(self) -> None:
        with self.assertRaises(ProductionIntakeNotWired) as ctx:
            extract_production_intake(
                {
                    "disposition": "accepted",
                    "objectId": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                    "sha256": "a" * 64,
                }
            )
        self.assertEqual(ctx.exception.code, "production_intake_not_wired")
        doc, ver, job = extract_production_intake(
            {
                "documentId": "11111111-1111-4111-8111-111111111111",
                "versionId": "22222222-2222-4222-8222-222222222222",
                "jobId": "33333333-3333-4333-8333-333333333333",
            }
        )
        self.assertTrue(doc.startswith("11111111"))
        self.assertTrue(ver.startswith("22222222"))
        self.assertTrue(job.startswith("33333333"))

    def test_suite_fixture_ids_resolve(self) -> None:
        suite = load_suite_manifest()
        fixtures = {f["id"] for f in load_json(FIXTURE_MANIFEST)["fixtures"]}
        for case in suite["formats"]:
            self.assertIn(case["fixtureId"], fixtures)
        for case in suite["security"]:
            fid = case.get("fixtureId")
            if fid:
                self.assertIn(fid, fixtures)

    def test_live_script_fail_closed_without_confirm(self) -> None:
        # Static inspection: script exits non-zero path via die when confirm unset.
        text = LIVE_SH.read_text(encoding="utf-8")
        self.assertIn("MARKHAND_E2E_CONFIRM", text)
        self.assertRegex(text, r'die .*MARKHAND_E2E_CONFIRM|die "set MARKHAND_E2E_CONFIRM')


def run_self_tests() -> None:
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(O04SelfTests)
    result = unittest.TextTestRunner(verbosity=1).run(suite)
    if not result.wasSuccessful():
        raise HarnessError("O04 self-tests failed")


def regenerate_hermetic_evidence() -> dict[str, Any]:
    report = run_hermetic_blocked_report()
    if report.get("claimsLiveVerticalSlice") is not False:
        raise HarnessError("hermetic evidence must not claim live vertical slice")
    blockers = " ".join(report.get("blockers") or [])
    for needle in (
        "Hermetic harness validation only",
        "production_intake_not_wired",
        "Docker",
    ):
        if needle not in blockers:
            raise HarnessError(f"hermetic evidence blockers missing {needle!r}")
    # Ensure reports exist and are redacted.
    for path in (REPORT_MD, REPORT_JSON):
        require_file(path)
        text = path.read_text(encoding="utf-8")
        leaks = assert_no_forbidden_evidence(text)
        if leaks:
            raise HarnessError(f"{path.name} failed redaction: {leaks}")
        if "claimsLiveVerticalSlice" in text or "claimsLiveVerticalSlice" in path.name:
            pass
        if "true" in text.lower() and "claimsliveverticalslice**: **true" in text.lower():
            raise HarnessError(f"{path.name} must not claim live vertical slice")
    md = REPORT_MD.read_text(encoding="utf-8")
    if "claimsLiveVerticalSlice`: **false**" not in md:
        raise HarnessError("markdown evidence must state claimsLiveVerticalSlice false")
    if "production_intake_not_wired" not in md:
        raise HarnessError("markdown evidence must list production_intake_not_wired")
    return report


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument(
        "--json-report",
        type=Path,
        default=REPORT_JSON,
        help="path written by hermetic evidence regeneration",
    )
    args = parser.parse_args()
    try:
        require_file(SUITE)
        require_file(FIXTURE_MANIFEST)
        require_file(FIXTURE_GEN)
        require_file(COMPOSE)
        require_file(POC_E2E_MANIFEST)
        validate_schemas()
        validate_suite_shape(load_json(SUITE))
        validate_fixtures()
        validate_scripts()
        validate_poc_manifest()
        if args.self_test:
            run_self_tests()
        report = regenerate_hermetic_evidence()
        # Optionally rewrite to requested path (already default).
        if args.json_report.resolve() != REPORT_JSON.resolve():
            args.json_report.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(REPORT_JSON, args.json_report)
    except HarnessError as error:
        print(f"P1B-O04 E2E validation FAILED: {error}", file=sys.stderr)
        return 1
    print(
        "P1B-O04 E2E hermetic validation OK "
        f"(formats={len(load_json(SUITE)['formats'])}, "
        f"security={len(load_json(SUITE)['security'])}, "
        f"fault={len(load_json(SUITE)['fault'])}, "
        f"claimsLiveVerticalSlice={report['claimsLiveVerticalSlice']})"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())