#!/usr/bin/env python3
"""Render a concise Markdown gate report from sanitized Phase 1C denial JSON.

Validates the manifest-run schema, compares payload digests against trusted
workflow inputs, applies fail-closed PASS/FAIL rules, and writes a human-readable
summary suitable for CI artifacts. Hermetic ``--self-test`` never reads real
repo integration output.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import unittest
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping, Sequence

REPORT_SCHEMA_VERSION = 1
GATE_1C12 = "1C-12"
GATE_1C13 = "1C-13"
GATE_1C13_STATUS = "not_run"
ENVIRONMENT_ID = "poc-compose"

SHA256_HEX = re.compile(r"^[0-9a-f]{64}$")
GIT_SHA_HEX = re.compile(r"^[0-9a-f]{40}$")

REQUIRED_TOP_LEVEL_KEYS = frozenset(
    {
        "schemaVersion",
        "gitShaFull",
        "manifestSha256",
        "executableCount",
        "naCount",
        "deferredCount",
        "binariesRun",
        "failures",
        "leakageCount",
        "findings",
        "redactionScan",
    }
)


@dataclass(frozen=True)
class RenderContext:
    expected_git_sha: str
    expected_manifest_sha256: str
    ci_run_url: str
    runner_exit_code: int
    teardown_exit_code: int


@dataclass(frozen=True)
class GateVerdict:
    passed: bool
    reasons: tuple[str, ...]


def _is_non_negative_int(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0


def validate_report_schema(payload: Mapping[str, Any]) -> list[str]:
    errors: list[str] = []
    missing = sorted(REQUIRED_TOP_LEVEL_KEYS - set(payload))
    if missing:
        errors.append(f"missing keys: {', '.join(missing)}")
        return errors

    if payload.get("schemaVersion") != REPORT_SCHEMA_VERSION:
        errors.append(f"schemaVersion must be exactly {REPORT_SCHEMA_VERSION}")

    git_sha = payload.get("gitShaFull")
    if not isinstance(git_sha, str) or not GIT_SHA_HEX.fullmatch(git_sha):
        errors.append("gitShaFull must be a 40-character lowercase hex string")

    manifest_sha = payload.get("manifestSha256")
    if not isinstance(manifest_sha, str) or not SHA256_HEX.fullmatch(manifest_sha):
        errors.append("manifestSha256 must be a 64-character lowercase hex string")

    for counter in ("executableCount", "naCount", "deferredCount", "leakageCount"):
        if not _is_non_negative_int(payload.get(counter)):
            errors.append(f"{counter} must be a non-negative int")

    if isinstance(payload.get("binariesRun"), bool) or not isinstance(
        payload.get("binariesRun"), list
    ) or not all(isinstance(item, str) for item in payload["binariesRun"]):
        errors.append("binariesRun must be a list of strings")

    if isinstance(payload.get("failures"), bool) or not isinstance(
        payload.get("failures"), list
    ) or not all(isinstance(item, str) for item in payload["failures"]):
        errors.append("failures must be a list of strings")

    redaction = payload.get("redactionScan")
    if not isinstance(redaction, dict):
        errors.append("redactionScan must be an object")
    else:
        passed = redaction.get("passed")
        findings = redaction.get("findings")
        if not isinstance(passed, bool):
            errors.append("redactionScan.passed must be a bool")
        if not isinstance(findings, list) or not all(isinstance(item, str) for item in findings):
            errors.append("redactionScan.findings must be a list of strings")
        elif isinstance(passed, bool) and isinstance(findings, list):
            if passed and findings:
                errors.append("redactionScan.passed true requires empty findings")
            if not passed and not findings:
                errors.append("redactionScan.passed false requires non-empty findings")

    findings = payload.get("findings")
    if not isinstance(findings, list):
        errors.append("findings must be a list")
    else:
        for index, finding in enumerate(findings):
            if not isinstance(finding, dict):
                errors.append(f"findings[{index}] must be an object")
                continue
            for key in ("category", "label", "hash"):
                value = finding.get(key)
                if not isinstance(value, str) or not value:
                    errors.append(f"findings[{index}].{key} must be a non-empty string")
            finding_hash = finding.get("hash")
            if isinstance(finding_hash, str) and not SHA256_HEX.fullmatch(finding_hash):
                errors.append(f"findings[{index}].hash must be lowercase hex")

    if _is_non_negative_int(payload.get("leakageCount")) and isinstance(findings, list):
        if payload["leakageCount"] != len(findings):
            errors.append("leakageCount must equal len(findings)")

    return errors


def build_fail_closed_report(
    *,
    expected_git_sha: str,
    expected_manifest_sha256: str,
    failure_message: str,
) -> dict[str, Any]:
    return {
        "schemaVersion": REPORT_SCHEMA_VERSION,
        "gitShaFull": expected_git_sha,
        "manifestSha256": expected_manifest_sha256,
        "executableCount": 0,
        "naCount": 0,
        "deferredCount": 0,
        "binariesRun": [],
        "failures": [failure_message],
        "leakageCount": 0,
        "findings": [],
        "redactionScan": {
            "passed": False,
            "findings": ["report synthesized without runner output"],
        },
    }


def evaluate_gate_verdict(
    payload: Mapping[str, Any],
    *,
    context: RenderContext,
) -> GateVerdict:
    reasons: list[str] = []
    if context.runner_exit_code != 0:
        reasons.append(f"runner exit code {context.runner_exit_code} != 0")
    if context.teardown_exit_code != 0:
        reasons.append(f"teardown exit code {context.teardown_exit_code} != 0")
    if payload.get("gitShaFull") != context.expected_git_sha:
        reasons.append("gitShaFull does not match trusted expected git SHA")
    if payload.get("manifestSha256") != context.expected_manifest_sha256:
        reasons.append("manifestSha256 does not match trusted expected manifest digest")
    if payload.get("failures"):
        reasons.append("failures non-empty")
    if int(payload.get("leakageCount", -1)) != 0:
        reasons.append(f"leakageCount={payload.get('leakageCount')}")
    if int(payload.get("deferredCount", -1)) != 0:
        reasons.append(f"deferredCount={payload.get('deferredCount')}")
    redaction = payload.get("redactionScan") or {}
    if not redaction.get("passed"):
        reasons.append("redactionScan.passed is false")
    return GateVerdict(passed=not reasons, reasons=tuple(reasons))


def render_markdown(
    payload: Mapping[str, Any],
    *,
    context: RenderContext,
) -> str:
    verdict = evaluate_gate_verdict(payload, context=context)
    status = "PASS" if verdict.passed else "FAIL"
    binaries = ", ".join(payload["binariesRun"]) if payload["binariesRun"] else "(none)"
    failure_lines = payload["failures"] or ["(none)"]
    redaction = payload["redactionScan"]
    redaction_status = "passed" if redaction.get("passed") else "failed"

    lines = [
        f"# Phase 1C denial gate report — {GATE_1C12}",
        "",
        "## Run identity",
        "",
        "| Field | Value |",
        "| --- | --- |",
        f"| Gate | `{GATE_1C12}` |",
        f"| Environment | `{ENVIRONMENT_ID}` |",
        f"| Git SHA (full) | `{payload['gitShaFull']}` |",
        f"| CI run | {context.ci_run_url} |",
        f"| Manifest SHA-256 | `{payload['manifestSha256']}` |",
        "",
        "## Manifest counts",
        "",
        "| Metric | Count |",
        "| --- | ---: |",
        f"| Executable | {payload['executableCount']} |",
        f"| N/A | {payload['naCount']} |",
        f"| Deferred | {payload['deferredCount']} |",
        "",
        "## Execution",
        "",
        f"- Binaries run: {binaries}",
        f"- Runner exit code: {context.runner_exit_code}",
        f"- Teardown exit code: {context.teardown_exit_code}",
        f"- Leakage findings: {payload['leakageCount']}",
        f"- Redaction scan: {redaction_status}",
        "",
        "## Failures",
        "",
    ]
    lines.extend(f"- {item}" for item in failure_lines)
    lines.extend(
        [
            "",
            "## Related gates",
            "",
            f"- `{GATE_1C13}`: `{GATE_1C13_STATUS}` (this job does not measure load/revoke/fairness thresholds)",
            "",
            "## Verdict",
            "",
            f"**{status}**",
        ]
    )
    if verdict.reasons:
        lines.append("")
        lines.append("Blocking reasons:")
        lines.extend(f"- {reason}" for reason in verdict.reasons)
    lines.append("")
    return "\n".join(lines)


def load_report(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def write_json(path: Path, payload: Mapping[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def ensure_input_report(
    input_path: Path,
    *,
    expected_git_sha: str,
    expected_manifest_sha256: str,
    failure_message: str,
) -> dict[str, Any]:
    if input_path.is_file():
        payload = load_report(input_path)
    else:
        payload = build_fail_closed_report(
            expected_git_sha=expected_git_sha,
            expected_manifest_sha256=expected_manifest_sha256,
            failure_message=failure_message,
        )
        write_json(input_path, payload)
    return payload


def render_file(
    *,
    input_path: Path,
    output_path: Path,
    context: RenderContext,
    failure_message: str = "manifest-run.json missing",
) -> GateVerdict:
    payload = ensure_input_report(
        input_path,
        expected_git_sha=context.expected_git_sha,
        expected_manifest_sha256=context.expected_manifest_sha256,
        failure_message=failure_message,
    )
    errors = validate_report_schema(payload)
    if errors:
        payload = build_fail_closed_report(
            expected_git_sha=context.expected_git_sha,
            expected_manifest_sha256=context.expected_manifest_sha256,
            failure_message=f"; ".join(errors),
        )
        write_json(input_path, payload)
        errors = validate_report_schema(payload)
        if errors:
            raise ValueError("; ".join(errors))

    markdown = render_markdown(payload, context=context)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(markdown, encoding="utf-8")
    return evaluate_gate_verdict(payload, context=context)


class RenderPhase1cDenialReportTests(unittest.TestCase):
    GIT_SHA = "a" * 40
    MANIFEST_SHA = "b" * 64

    def sample_payload(self) -> dict[str, Any]:
        return {
            "schemaVersion": 1,
            "gitShaFull": self.GIT_SHA,
            "manifestSha256": self.MANIFEST_SHA,
            "executableCount": 3,
            "naCount": 2,
            "deferredCount": 0,
            "binariesRun": ["multi_org_denial", "api_http_contracts"],
            "failures": [],
            "leakageCount": 0,
            "findings": [],
            "redactionScan": {"passed": True, "findings": []},
        }

    def sample_context(
        self,
        *,
        runner_exit_code: int = 0,
        teardown_exit_code: int = 0,
    ) -> RenderContext:
        return RenderContext(
            expected_git_sha=self.GIT_SHA,
            expected_manifest_sha256=self.MANIFEST_SHA,
            ci_run_url="https://github.com/example/markhand/actions/runs/123",
            runner_exit_code=runner_exit_code,
            teardown_exit_code=teardown_exit_code,
        )

    def test_validate_schema_rejects_missing_keys(self) -> None:
        errors = validate_report_schema({"schemaVersion": 1})
        self.assertTrue(any("missing keys" in error for error in errors))

    def test_validate_schema_rejects_bool_counters_and_bad_hashes(self) -> None:
        payload = self.sample_payload()
        payload["executableCount"] = True
        self.assertTrue(any("executableCount" in error for error in validate_report_schema(payload)))

        payload = self.sample_payload()
        payload["gitShaFull"] = "A" * 40
        self.assertTrue(any("gitShaFull" in error for error in validate_report_schema(payload)))

        payload = self.sample_payload()
        payload["schemaVersion"] = 2
        self.assertTrue(any("schemaVersion must be exactly 1" in error for error in validate_report_schema(payload)))

    def test_validate_schema_rejects_leakage_and_redaction_inconsistency(self) -> None:
        payload = self.sample_payload()
        payload["leakageCount"] = 1
        self.assertTrue(any("leakageCount must equal len(findings)" in error for error in validate_report_schema(payload)))

        payload = self.sample_payload()
        payload["redactionScan"] = {"passed": True, "findings": ["token"]}
        self.assertTrue(any("redactionScan.passed true requires empty findings" in error for error in validate_report_schema(payload)))

        payload = self.sample_payload()
        payload["redactionScan"] = {"passed": False, "findings": []}
        self.assertTrue(any("redactionScan.passed false requires non-empty findings" in error for error in validate_report_schema(payload)))

    def test_pass_verdict_requires_all_clear_signals(self) -> None:
        verdict = evaluate_gate_verdict(self.sample_payload(), context=self.sample_context())
        self.assertTrue(verdict.passed)

    def test_fail_when_metadata_or_lifecycle_mismatch(self) -> None:
        payload = self.sample_payload()
        payload["gitShaFull"] = "c" * 40
        verdict = evaluate_gate_verdict(payload, context=self.sample_context())
        self.assertFalse(verdict.passed)
        self.assertIn("trusted expected git SHA", verdict.reasons[0])

        payload = self.sample_payload()
        verdict = evaluate_gate_verdict(
            payload,
            context=self.sample_context(runner_exit_code=1, teardown_exit_code=2),
        )
        self.assertFalse(verdict.passed)
        self.assertIn("runner exit code 1", verdict.reasons[0])
        self.assertIn("teardown exit code 2", verdict.reasons[1])

    def test_render_includes_required_identity_and_verdict(self) -> None:
        markdown = render_markdown(
            self.sample_payload(),
            context=self.sample_context(),
        )
        self.assertIn(self.GIT_SHA, markdown)
        self.assertIn("https://github.com/example/markhand/actions/runs/123", markdown)
        self.assertIn(f"`{ENVIRONMENT_ID}`", markdown)
        self.assertIn(f"`{GATE_1C12}`", markdown)
        self.assertIn("Teardown exit code: 0", markdown)
        self.assertIn("**PASS**", markdown)
        self.assertIn(f"`{GATE_1C13}`: `{GATE_1C13_STATUS}`", markdown)

    def test_render_fail_is_unambiguous(self) -> None:
        payload = self.sample_payload()
        payload["failures"] = ["binary multi_org_denial exited 101"]
        markdown = render_markdown(
            payload,
            context=self.sample_context(runner_exit_code=1),
        )
        self.assertIn("**FAIL**", markdown)
        self.assertIn("failures non-empty", markdown)

    def test_fail_closed_report_is_schema_valid_and_renders_fail(self) -> None:
        payload = build_fail_closed_report(
            expected_git_sha=self.GIT_SHA,
            expected_manifest_sha256=self.MANIFEST_SHA,
            failure_message="manifest-run.json missing",
        )
        self.assertEqual([], validate_report_schema(payload))
        markdown = render_markdown(payload, context=self.sample_context(runner_exit_code=1))
        self.assertIn("**FAIL**", markdown)
        self.assertIn("redactionScan.passed is false", markdown)


def run_self_tests() -> int:
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(RenderPhase1cDenialReportTests)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1


def _validate_trusted_hex(name: str, value: str, pattern: re.Pattern[str], length: int) -> None:
    if not pattern.fullmatch(value):
        raise ValueError(f"{name} must be a {length}-character lowercase hex string")


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, help="Sanitized manifest-run.json path")
    parser.add_argument("--output", type=Path, help="Markdown output path")
    parser.add_argument(
        "--expected-git-sha",
        help="Trusted 40-char lowercase git SHA for this run",
    )
    parser.add_argument(
        "--expected-manifest-sha256",
        help="Trusted 64-char lowercase manifest SHA-256",
    )
    parser.add_argument("--ci-run-url", help="CI workflow run URL")
    parser.add_argument(
        "--runner-exit-code",
        type=int,
        default=0,
        help="Exit code from scripts/run-phase1c-denial-suite.py",
    )
    parser.add_argument(
        "--teardown-exit-code",
        type=int,
        default=0,
        help="Exit code from POC stack teardown",
    )
    parser.add_argument(
        "--missing-input-reason",
        default="manifest-run.json missing",
        help="Failure reason written when --input does not exist",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="Run hermetic renderer contract tests",
    )
    args = parser.parse_args(argv)

    if args.self_test:
        return run_self_tests()

    if args.input is None or args.output is None:
        parser.error("--input and --output are required unless --self-test is used")
    if not args.expected_git_sha or not args.expected_manifest_sha256 or not args.ci_run_url:
        parser.error(
            "--expected-git-sha, --expected-manifest-sha256, and --ci-run-url are required"
        )

    try:
        _validate_trusted_hex("expected git sha", args.expected_git_sha, GIT_SHA_HEX, 40)
        _validate_trusted_hex(
            "expected manifest sha256",
            args.expected_manifest_sha256,
            SHA256_HEX,
            64,
        )
        context = RenderContext(
            expected_git_sha=args.expected_git_sha,
            expected_manifest_sha256=args.expected_manifest_sha256,
            ci_run_url=args.ci_run_url,
            runner_exit_code=args.runner_exit_code,
            teardown_exit_code=args.teardown_exit_code,
        )
        verdict = render_file(
            input_path=args.input.resolve(),
            output_path=args.output.resolve(),
            context=context,
            failure_message=args.missing_input_reason,
        )
    except (OSError, json.JSONDecodeError, ValueError) as exc:
        print(f"render failed: {exc}", file=sys.stderr)
        return 2

    if not verdict.passed:
        print("gate verdict: FAIL", file=sys.stderr)
        for reason in verdict.reasons:
            print(f"- {reason}", file=sys.stderr)
        return 1
    print("gate verdict: PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
