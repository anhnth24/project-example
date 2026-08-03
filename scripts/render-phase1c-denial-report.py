#!/usr/bin/env python3
"""Render a concise Markdown gate report from sanitized Phase 1C denial JSON.

Validates the manifest-run schema, applies fail-closed PASS/FAIL rules, and
writes a human-readable summary suitable for CI artifacts. Hermetic ``--self-test``
never reads real repo integration output.
"""

from __future__ import annotations

import argparse
import json
import sys
import unittest
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping, Sequence

REPORT_SCHEMA_VERSION = 1
GATE_1C12 = "1C-12"
GATE_1C13 = "1C-13"
ENVIRONMENT_ID = "poc-compose"

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
    gate: str
    environment_id: str
    git_ref: str
    ci_run_url: str
    runner_exit_code: int
    gate_1c13_status: str = "not_run"


@dataclass(frozen=True)
class GateVerdict:
    passed: bool
    reasons: tuple[str, ...]


def validate_report_schema(payload: Mapping[str, Any]) -> list[str]:
    errors: list[str] = []
    missing = sorted(REQUIRED_TOP_LEVEL_KEYS - set(payload))
    if missing:
        errors.append(f"missing keys: {', '.join(missing)}")
        return errors

    schema_version = payload.get("schemaVersion")
    if not isinstance(schema_version, int) or schema_version < REPORT_SCHEMA_VERSION:
        errors.append(
            f"schemaVersion must be int >= {REPORT_SCHEMA_VERSION}, got {schema_version!r}"
        )

    git_sha = payload.get("gitShaFull")
    if not isinstance(git_sha, str) or len(git_sha) != 40:
        errors.append("gitShaFull must be a 40-character string")

    manifest_sha = payload.get("manifestSha256")
    if not isinstance(manifest_sha, str) or len(manifest_sha) != 64:
        errors.append("manifestSha256 must be a 64-character hex string")

    for counter in ("executableCount", "naCount", "deferredCount", "leakageCount"):
        if not isinstance(payload.get(counter), int) or payload[counter] < 0:
            errors.append(f"{counter} must be a non-negative int")

    if not isinstance(payload.get("binariesRun"), list) or not all(
        isinstance(item, str) for item in payload["binariesRun"]
    ):
        errors.append("binariesRun must be a list of strings")

    if not isinstance(payload.get("failures"), list) or not all(
        isinstance(item, str) for item in payload["failures"]
    ):
        errors.append("failures must be a list of strings")

    redaction = payload.get("redactionScan")
    if not isinstance(redaction, dict):
        errors.append("redactionScan must be an object")
    else:
        if not isinstance(redaction.get("passed"), bool):
            errors.append("redactionScan.passed must be a bool")
        findings = redaction.get("findings")
        if not isinstance(findings, list) or not all(isinstance(item, str) for item in findings):
            errors.append("redactionScan.findings must be a list of strings")

    findings = payload.get("findings")
    if not isinstance(findings, list):
        errors.append("findings must be a list")
    return errors


def evaluate_gate_verdict(
    payload: Mapping[str, Any],
    *,
    runner_exit_code: int,
) -> GateVerdict:
    reasons: list[str] = []
    if runner_exit_code != 0:
        reasons.append(f"runner exit code {runner_exit_code} != 0")
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
    verdict = evaluate_gate_verdict(payload, runner_exit_code=context.runner_exit_code)
    status = "PASS" if verdict.passed else "FAIL"
    binaries = ", ".join(payload["binariesRun"]) if payload["binariesRun"] else "(none)"
    failure_lines = payload["failures"] or ["(none)"]
    redaction = payload["redactionScan"]
    redaction_status = "passed" if redaction.get("passed") else "failed"

    lines = [
        f"# Phase 1C denial gate report — {context.gate}",
        "",
        "## Run identity",
        "",
        f"| Field | Value |",
        f"| --- | --- |",
        f"| Gate | `{context.gate}` |",
        f"| Environment | `{context.environment_id}` |",
        f"| Git SHA (full) | `{payload['gitShaFull']}` |",
        f"| Git ref | `{context.git_ref}` |",
        f"| CI run | {context.ci_run_url} |",
        f"| Manifest SHA-256 | `{payload['manifestSha256']}` |",
        "",
        "## Manifest counts",
        "",
        f"| Metric | Count |",
        f"| --- | ---: |",
        f"| Executable | {payload['executableCount']} |",
        f"| N/A | {payload['naCount']} |",
        f"| Deferred | {payload['deferredCount']} |",
        "",
        "## Execution",
        "",
        f"- Binaries run: {binaries}",
        f"- Runner exit code: {context.runner_exit_code}",
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
            f"- `{GATE_1C13}`: `{context.gate_1c13_status}` (this job does not measure load/revoke/fairness thresholds)",
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


def render_file(
    *,
    input_path: Path,
    output_path: Path,
    context: RenderContext,
) -> GateVerdict:
    payload = load_report(input_path)
    errors = validate_report_schema(payload)
    if errors:
        raise ValueError("; ".join(errors))
    markdown = render_markdown(payload, context=context)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(markdown, encoding="utf-8")
    return evaluate_gate_verdict(payload, runner_exit_code=context.runner_exit_code)


class RenderPhase1cDenialReportTests(unittest.TestCase):
    def sample_payload(self) -> dict[str, Any]:
        return {
            "schemaVersion": 1,
            "gitShaFull": "a" * 40,
            "manifestSha256": "b" * 64,
            "executableCount": 3,
            "naCount": 2,
            "deferredCount": 0,
            "binariesRun": ["multi_org_denial", "api_http_contracts"],
            "failures": [],
            "leakageCount": 0,
            "findings": [],
            "redactionScan": {"passed": True, "findings": []},
        }

    def sample_context(self, *, runner_exit_code: int = 0) -> RenderContext:
        return RenderContext(
            gate=GATE_1C12,
            environment_id=ENVIRONMENT_ID,
            git_ref="cursor/phase1c-deployed-evidence-06b6",
            ci_run_url="https://github.com/example/markhand/actions/runs/123",
            runner_exit_code=runner_exit_code,
        )

    def test_validate_schema_rejects_missing_keys(self) -> None:
        errors = validate_report_schema({"schemaVersion": 1})
        self.assertTrue(any("missing keys" in error for error in errors))

    def test_pass_verdict_requires_all_clear_signals(self) -> None:
        payload = self.sample_payload()
        verdict = evaluate_gate_verdict(payload, runner_exit_code=0)
        self.assertTrue(verdict.passed)

    def test_fail_when_runner_exit_nonzero(self) -> None:
        payload = self.sample_payload()
        verdict = evaluate_gate_verdict(payload, runner_exit_code=2)
        self.assertFalse(verdict.passed)
        self.assertIn("runner exit code 2 != 0", verdict.reasons)

    def test_fail_when_leakage_or_redaction_or_deferred(self) -> None:
        payload = self.sample_payload()
        payload["leakageCount"] = 1
        self.assertFalse(evaluate_gate_verdict(payload, runner_exit_code=0).passed)

        payload = self.sample_payload()
        payload["deferredCount"] = 1
        self.assertFalse(evaluate_gate_verdict(payload, runner_exit_code=0).passed)

        payload = self.sample_payload()
        payload["redactionScan"] = {"passed": False, "findings": ["Bearer token"]}
        self.assertFalse(evaluate_gate_verdict(payload, runner_exit_code=0).passed)

    def test_render_includes_required_identity_and_verdict(self) -> None:
        markdown = render_markdown(
            self.sample_payload(),
            context=self.sample_context(),
        )
        self.assertIn("a" * 40, markdown)
        self.assertIn("cursor/phase1c-deployed-evidence-06b6", markdown)
        self.assertIn("https://github.com/example/markhand/actions/runs/123", markdown)
        self.assertIn(f"`{ENVIRONMENT_ID}`", markdown)
        self.assertIn(f"`{GATE_1C12}`", markdown)
        self.assertIn("Executable | 3", markdown)
        self.assertIn("multi_org_denial", markdown)
        self.assertIn("**PASS**", markdown)
        self.assertIn(f"`{GATE_1C13}`: `not_run`", markdown)

    def test_render_fail_is_unambiguous(self) -> None:
        payload = self.sample_payload()
        payload["failures"] = ["binary multi_org_denial exited 101"]
        markdown = render_markdown(
            payload,
            context=self.sample_context(runner_exit_code=1),
        )
        self.assertIn("**FAIL**", markdown)
        self.assertIn("failures non-empty", markdown)


def run_self_tests() -> int:
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(RenderPhase1cDenialReportTests)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, help="Sanitized manifest-run.json path")
    parser.add_argument("--output", type=Path, help="Markdown output path")
    parser.add_argument("--gate", default=GATE_1C12, help="Gate identifier (default: 1C-12)")
    parser.add_argument(
        "--environment-id",
        default=ENVIRONMENT_ID,
        help="Environment identifier (default: poc-compose)",
    )
    parser.add_argument("--git-ref", default="", help="Branch or ref under test")
    parser.add_argument("--ci-run-url", default="", help="CI workflow run URL")
    parser.add_argument(
        "--runner-exit-code",
        type=int,
        default=0,
        help="Exit code from scripts/run-phase1c-denial-suite.py",
    )
    parser.add_argument(
        "--gate-1c13-status",
        default="not_run",
        help="Explicit status for gate 1C-13 (default: not_run)",
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

    try:
        verdict = render_file(
            input_path=args.input.resolve(),
            output_path=args.output.resolve(),
            context=RenderContext(
                gate=args.gate,
                environment_id=args.environment_id,
                git_ref=args.git_ref,
                ci_run_url=args.ci_run_url,
                runner_exit_code=args.runner_exit_code,
                gate_1c13_status=args.gate_1c13_status,
            ),
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
