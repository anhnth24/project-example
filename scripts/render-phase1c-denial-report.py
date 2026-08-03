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
import os
import re
import sys
import tempfile
import unittest
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping, Sequence
from unittest.mock import patch

REPORT_SCHEMA_VERSION = 1
GATE_1C12 = "1C-12"
GATE_1C13 = "1C-13"
GATE_1C13_STATUS = "not_run"
ENVIRONMENT_ID = "poc-compose"

SHA256_HEX = re.compile(r"^[0-9a-f]{64}$")
GIT_SHA_HEX = re.compile(r"^[0-9a-f]{40}$")
GIT_REF_SAFE = re.compile(r"^[A-Za-z0-9._/@-]{1,256}$")

FAILURE_RUNNER_OUTPUT_MISSING = "runner output missing"
FAILURE_RUNNER_OUTPUT_MALFORMED = "runner output malformed"
FAILURE_RUNNER_OUTPUT_INVALID_ROOT = "runner output invalid root"
FAILURE_RUNNER_OUTPUT_SCHEMA_INVALID = "runner output schema invalid"
FAILURE_RUNNER_STEP_INCOMPLETE = "runner step incomplete"

ALLOWED_FAILURE_MESSAGES = frozenset(
    {
        FAILURE_RUNNER_OUTPUT_MISSING,
        FAILURE_RUNNER_OUTPUT_MALFORMED,
        FAILURE_RUNNER_OUTPUT_INVALID_ROOT,
        FAILURE_RUNNER_OUTPUT_SCHEMA_INVALID,
        FAILURE_RUNNER_STEP_INCOMPLETE,
    }
)

BEARER_RE = re.compile(r"(?i)\bBearer\s+[A-Za-z0-9._\-+=/]{8,}")
JWT_RE = re.compile(
    r"\beyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\b"
)
ASSIGN_SECRET_RE = re.compile(
    r"(?i)\b(password|passwd|secret|token|api[_-]?key|access[_-]?key|private[_-]?key)\s*[=:]\s*"
    r"([^\s'\"\\]+|'[^']*'|\"[^\"]*\")"
)
DB_URL_RE = re.compile(r"(?i)\b(postgres(?:ql)?|mysql|mongodb|redis)://[^\s'\"]+")
CI_SECRET_RE = re.compile(
    r"(?i)\b(GITHUB_TOKEN|AWS_SECRET_ACCESS_KEY|NPM_TOKEN|PYPI_API_TOKEN)\s*[=:]\s*\S+"
)
PEM_RE = re.compile(
    r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?-----END [A-Z0-9 ]*PRIVATE KEY-----",
    re.DOTALL,
)

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
REDACTION_KEYS = frozenset({"passed", "findings"})
FINDING_KEYS = frozenset({"category", "label", "hash"})


@dataclass(frozen=True)
class RenderContext:
    expected_git_sha: str
    expected_manifest_sha256: str
    expected_git_ref: str
    ci_run_url: str
    runner_exit_code: int
    teardown_exit_code: int


@dataclass(frozen=True)
class GateVerdict:
    passed: bool
    reasons: tuple[str, ...]


def scan_for_secret_shapes(text: str) -> list[str]:
    labels: list[str] = []
    if BEARER_RE.search(text):
        labels.append("bearer_token")
    if JWT_RE.search(text):
        labels.append("jwt")
    if ASSIGN_SECRET_RE.search(text):
        labels.append("assignment_secret")
    if DB_URL_RE.search(text):
        labels.append("database_url")
    if CI_SECRET_RE.search(text):
        labels.append("ci_secret_assignment")
    if PEM_RE.search(text):
        labels.append("pem_private_key")
    return labels


def validate_failure_message(message: str) -> None:
    if message not in ALLOWED_FAILURE_MESSAGES:
        raise ValueError("failure message must be a fixed categorical label")


def assert_artifacts_safe(payload: Mapping[str, Any], markdown: str) -> None:
    serialized = json.dumps(payload, sort_keys=True)
    residuals = scan_for_secret_shapes(serialized) + scan_for_secret_shapes(markdown)
    if residuals:
        raise ValueError(
            "fallback artifact failed secret residual scan: "
            + ", ".join(sorted(set(residuals)))
        )


def _is_non_negative_int(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0


def _is_exact_schema_version(value: object) -> bool:
    return type(value) is int and value == REPORT_SCHEMA_VERSION


def validate_git_ref(value: str) -> None:
    if not value or "\n" in value or "\r" in value:
        raise ValueError("expected git ref must be a single non-empty line")
    if len(value) > 256:
        raise ValueError("expected git ref exceeds 256 characters")
    if not GIT_REF_SAFE.fullmatch(value):
        raise ValueError("expected git ref contains unsafe characters")


def markdown_escape_cell(value: str) -> str:
    return value.replace("\\", "\\\\").replace("|", "\\|")


def validate_report_schema(payload: Mapping[str, Any]) -> list[str]:
    errors: list[str] = []

    extra_top = sorted(set(payload.keys()) - REQUIRED_TOP_LEVEL_KEYS)
    if extra_top:
        errors.append(f"unexpected top-level keys: {', '.join(extra_top)}")

    missing = sorted(REQUIRED_TOP_LEVEL_KEYS - set(payload))
    if missing:
        errors.append(f"missing keys: {', '.join(missing)}")
        return errors

    schema_version = payload.get("schemaVersion")
    if isinstance(schema_version, bool) or not _is_exact_schema_version(schema_version):
        errors.append(f"schemaVersion must be exactly int {REPORT_SCHEMA_VERSION}")

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
        extra_redaction = sorted(set(redaction.keys()) - REDACTION_KEYS)
        if extra_redaction:
            errors.append(
                f"redactionScan unexpected keys: {', '.join(extra_redaction)}"
            )
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
            extra_finding = sorted(set(finding.keys()) - FINDING_KEYS)
            if extra_finding:
                errors.append(
                    f"findings[{index}] unexpected keys: {', '.join(extra_finding)}"
                )
            for key in FINDING_KEYS:
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
    validate_failure_message(failure_message)
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
    ref_cell = markdown_escape_cell(context.expected_git_ref)

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
        f"| Git ref | `{ref_cell}` |",
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


def parse_report_json(text: str) -> tuple[dict[str, Any] | None, str | None]:
    try:
        raw = json.loads(text)
    except json.JSONDecodeError:
        return None, FAILURE_RUNNER_OUTPUT_MALFORMED
    if isinstance(raw, list):
        return None, FAILURE_RUNNER_OUTPUT_INVALID_ROOT
    if not isinstance(raw, dict):
        return None, FAILURE_RUNNER_OUTPUT_INVALID_ROOT
    return raw, None


def read_report_file(path: Path) -> tuple[dict[str, Any] | None, str | None]:
    if not path.is_file():
        return None, FAILURE_RUNNER_OUTPUT_MISSING
    return parse_report_json(path.read_text(encoding="utf-8"))


def write_json(path: Path, payload: Mapping[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def write_json_atomic(path: Path, payload: Mapping[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    text = json.dumps(payload, indent=2, sort_keys=True) + "\n"
    temporary = synthesized_temp_path(path)
    try:
        temporary.write_text(text, encoding="utf-8")
        os.replace(temporary, path)
    finally:
        if temporary.is_file():
            temporary.unlink(missing_ok=True)


def synthesized_temp_path(input_path: Path) -> Path:
    return input_path.with_name(f".{input_path.name}.tmp-{os.getpid()}")


def purge_synthesized_temp_files(input_path: Path) -> None:
    prefix = f".{input_path.name}.tmp-"
    try:
        for entry in input_path.parent.iterdir():
            if entry.name.startswith(prefix):
                entry.unlink(missing_ok=True)
    except OSError:
        pass


def purge_allowlisted_artifacts(input_path: Path, output_path: Path) -> None:
    for path in (input_path, output_path):
        try:
            path.unlink(missing_ok=True)
        except OSError:
            pass


def purge_synthesized_fallback_artifacts(input_path: Path, output_path: Path) -> None:
    purge_allowlisted_artifacts(input_path, output_path)
    purge_synthesized_temp_files(input_path)


SYNTHESIZED_FALLBACK_FAILURES = (OSError, ValueError, TypeError, json.JSONDecodeError)


def commit_synthesized_input(
    input_path: Path,
    payload: Mapping[str, Any],
) -> None:
    errors = validate_report_schema(payload)
    if errors:
        raise ValueError("synthesized fallback report failed schema validation")
    write_json_atomic(input_path, payload)


def resolve_input_report(
    input_path: Path,
    *,
    expected_git_sha: str,
    expected_manifest_sha256: str,
    input_failure_category: str,
) -> tuple[dict[str, Any], bool]:
    validate_failure_message(input_failure_category)
    payload, read_error = read_report_file(input_path)
    if read_error is not None:
        category = read_error
        payload = build_fail_closed_report(
            expected_git_sha=expected_git_sha,
            expected_manifest_sha256=expected_manifest_sha256,
            failure_message=category,
        )
        return payload, True

    if validate_report_schema(payload):
        payload = build_fail_closed_report(
            expected_git_sha=expected_git_sha,
            expected_manifest_sha256=expected_manifest_sha256,
            failure_message=FAILURE_RUNNER_OUTPUT_SCHEMA_INVALID,
        )
        return payload, True

    return payload, False


def render_file(
    *,
    input_path: Path,
    output_path: Path,
    context: RenderContext,
    input_failure_category: str = FAILURE_RUNNER_OUTPUT_MISSING,
) -> GateVerdict:
    payload, synthesized = resolve_input_report(
        input_path,
        expected_git_sha=context.expected_git_sha,
        expected_manifest_sha256=context.expected_manifest_sha256,
        input_failure_category=input_failure_category,
    )

    try:
        if synthesized:
            commit_synthesized_input(input_path, payload)

        errors = validate_report_schema(payload)
        if errors:
            raise ValueError("synthesized fallback report failed schema validation")

        markdown = render_markdown(payload, context=context)
        if synthesized:
            assert_artifacts_safe(payload, markdown)
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_text(markdown, encoding="utf-8")
    except SYNTHESIZED_FALLBACK_FAILURES:
        if synthesized:
            purge_synthesized_fallback_artifacts(input_path, output_path)
        raise

    return evaluate_gate_verdict(payload, context=context)


class RenderPhase1cDenialReportTests(unittest.TestCase):
    GIT_SHA = "a" * 40
    MANIFEST_SHA = "b" * 64
    GIT_REF = "cursor/phase1c-deployed-evidence-06b6"

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
        git_ref: str | None = None,
    ) -> RenderContext:
        return RenderContext(
            expected_git_sha=self.GIT_SHA,
            expected_manifest_sha256=self.MANIFEST_SHA,
            expected_git_ref=git_ref or self.GIT_REF,
            ci_run_url="https://github.com/anhnth24/project-example/actions/runs/123",
            runner_exit_code=runner_exit_code,
            teardown_exit_code=teardown_exit_code,
        )

    def test_validate_schema_rejects_missing_keys(self) -> None:
        errors = validate_report_schema({"schemaVersion": 1})
        self.assertTrue(any("missing keys" in error for error in errors))

    def test_validate_schema_rejects_bool_schema_version_and_wrong_roots(self) -> None:
        payload = self.sample_payload()
        payload["schemaVersion"] = True
        errors = validate_report_schema(payload)
        self.assertTrue(any("schemaVersion must be exactly int 1" in error for error in errors))

        payload, error = parse_report_json("[]")
        self.assertIsNone(payload)
        self.assertEqual(FAILURE_RUNNER_OUTPUT_INVALID_ROOT, error)

        payload, error = parse_report_json('"scalar"')
        self.assertIsNone(payload)
        self.assertEqual(FAILURE_RUNNER_OUTPUT_INVALID_ROOT, error)

    def test_validate_schema_rejects_unexpected_keys(self) -> None:
        payload = self.sample_payload()
        payload["extra"] = 1
        payload["redactionScan"]["token"] = "secret"
        payload["findings"] = [{"category": "x", "label": "y", "hash": "c" * 64, "needle": "z"}]
        errors = validate_report_schema(payload)
        self.assertTrue(any("unexpected top-level keys: extra" in error for error in errors))
        self.assertTrue(any("redactionScan unexpected keys" in error for error in errors))
        self.assertTrue(any("findings[0] unexpected keys" in error for error in errors))

    def test_validate_schema_rejects_bool_counters_and_bad_hashes(self) -> None:
        payload = self.sample_payload()
        payload["executableCount"] = True
        self.assertTrue(any("executableCount" in error for error in validate_report_schema(payload)))

        payload = self.sample_payload()
        payload["gitShaFull"] = "A" * 40
        self.assertTrue(any("gitShaFull" in error for error in validate_report_schema(payload)))

    def test_validate_git_ref_rejects_unsafe_values(self) -> None:
        with self.assertRaises(ValueError):
            validate_git_ref("branch|inject")
        with self.assertRaises(ValueError):
            validate_git_ref("line\nbreak")

    def test_render_escapes_ref_for_markdown_table(self) -> None:
        context = self.sample_context(git_ref="feature/test|safe")
        markdown = render_markdown(self.sample_payload(), context=context)
        self.assertIn("feature/test\\|safe", markdown)

    def test_pass_verdict_requires_all_clear_signals(self) -> None:
        verdict = evaluate_gate_verdict(self.sample_payload(), context=self.sample_context())
        self.assertTrue(verdict.passed)

    def test_render_includes_trusted_ref_in_identity(self) -> None:
        markdown = render_markdown(self.sample_payload(), context=self.sample_context())
        self.assertIn(f"| Git ref | `{self.GIT_REF}` |", markdown)

    def test_malformed_json_is_replaced_with_schema_valid_fail_closed_artifact(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            input_path = root / "manifest-run.json"
            output_path = root / "phase1c-denial-report.md"
            input_path.write_text("{not json", encoding="utf-8")
            verdict = render_file(
                input_path=input_path,
                output_path=output_path,
                context=self.sample_context(runner_exit_code=1),
            )
            self.assertFalse(verdict.passed)
            self.assertEqual([], validate_report_schema(json.loads(input_path.read_text())))
            self.assertNotIn("{not json", input_path.read_text())
            self.assertIn("**FAIL**", output_path.read_text())

    def test_list_root_is_replaced_and_renders_fail_markdown(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            input_path = root / "manifest-run.json"
            output_path = root / "phase1c-denial-report.md"
            input_path.write_text("[]", encoding="utf-8")
            render_file(
                input_path=input_path,
                output_path=output_path,
                context=self.sample_context(runner_exit_code=1),
            )
            self.assertIn("**FAIL**", output_path.read_text())
            self.assertEqual([], validate_report_schema(json.loads(input_path.read_text())))

    def test_malicious_unexpected_key_does_not_leak_into_artifacts(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            input_path = root / "manifest-run.json"
            output_path = root / "phase1c-denial-report.md"
            payload = self.sample_payload()
            payload["Bearer SUPERSECRET123"] = "ignored-value"
            input_path.write_text(json.dumps(payload), encoding="utf-8")
            render_file(
                input_path=input_path,
                output_path=output_path,
                context=self.sample_context(runner_exit_code=1),
            )
            json_text = input_path.read_text(encoding="utf-8")
            markdown = output_path.read_text(encoding="utf-8")
            self.assertNotIn("Bearer SUPERSECRET123", json_text)
            self.assertNotIn("SUPERSECRET123", json_text)
            self.assertNotIn("Bearer SUPERSECRET123", markdown)
            self.assertNotIn("SUPERSECRET123", markdown)
            self.assertIn(FAILURE_RUNNER_OUTPUT_SCHEMA_INVALID, json_text)

    JWT_SHAPED_REF = (
        "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9."
        "eyJzdWIiOiIxMjM0NTY3ODkwIn0."
        "dozjgNryP4J3jVmNHl0w5N_XgL0YX3gc8"
    )

    def _assert_no_untrusted_secret_at_allowlisted_paths(
        self,
        input_path: Path,
        output_path: Path,
    ) -> None:
        for path in (input_path, output_path):
            if not path.is_file():
                continue
            text = path.read_text(encoding="utf-8")
            self.assertNotIn("SUPERSECRET123", text)
            self.assertNotIn("Bearer SUPERSECRET123", text)

    def _assert_no_untrusted_secret_in_temp_siblings(self, input_path: Path) -> None:
        prefix = f".{input_path.name}.tmp-"
        for entry in input_path.parent.iterdir():
            if not entry.name.startswith(prefix):
                continue
            if entry.is_file():
                text = entry.read_text(encoding="utf-8")
                self.assertNotIn("SUPERSECRET123", text)
                self.assertNotIn("Bearer SUPERSECRET123", text)
            self.fail(f"unexpected synthesized temp sibling remained: {entry}")

    def _write_schema_invalid_secret_input(self, input_path: Path) -> None:
        payload = self.sample_payload()
        payload["Bearer SUPERSECRET123"] = "ignored-value"
        input_path.write_text(json.dumps(payload), encoding="utf-8")

    def test_atomic_temp_write_failure_purges_untrusted_input(self) -> None:
        original_write_text = Path.write_text

        def failing_write_text(self: Path, *args: object, **kwargs: object) -> int:
            if self.name.startswith(".manifest-run.json.tmp-"):
                raise OSError("injected temp write failure")
            return original_write_text(self, *args, **kwargs)

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            input_path = root / "manifest-run.json"
            output_path = root / "phase1c-denial-report.md"
            self._write_schema_invalid_secret_input(input_path)
            with patch.object(Path, "write_text", failing_write_text):
                with self.assertRaises(OSError):
                    render_file(
                        input_path=input_path,
                        output_path=output_path,
                        context=self.sample_context(runner_exit_code=1),
                    )
            self._assert_no_untrusted_secret_at_allowlisted_paths(input_path, output_path)
            self._assert_no_untrusted_secret_in_temp_siblings(input_path)
            self.assertFalse(input_path.is_file())

    def test_atomic_replace_failure_purges_untrusted_input(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            input_path = root / "manifest-run.json"
            output_path = root / "phase1c-denial-report.md"
            self._write_schema_invalid_secret_input(input_path)
            with patch(f"{__name__}.os.replace", side_effect=OSError("injected replace failure")):
                with self.assertRaises(OSError):
                    render_file(
                        input_path=input_path,
                        output_path=output_path,
                        context=self.sample_context(runner_exit_code=1),
                    )
            self._assert_no_untrusted_secret_at_allowlisted_paths(input_path, output_path)
            self._assert_no_untrusted_secret_in_temp_siblings(input_path)
            self.assertFalse(input_path.is_file())

    def test_jwt_trusted_ref_scan_failure_purges_untrusted_input(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            input_path = root / "manifest-run.json"
            output_path = root / "phase1c-denial-report.md"
            payload = self.sample_payload()
            payload["Bearer SUPERSECRET123"] = "ignored-value"
            input_path.write_text(json.dumps(payload), encoding="utf-8")
            with self.assertRaises(ValueError):
                render_file(
                    input_path=input_path,
                    output_path=output_path,
                    context=self.sample_context(
                        runner_exit_code=1,
                        git_ref=self.JWT_SHAPED_REF,
                    ),
                )
            self._assert_no_untrusted_secret_at_allowlisted_paths(input_path, output_path)
            self._assert_no_untrusted_secret_in_temp_siblings(input_path)
            if input_path.is_file():
                self.assertEqual(
                    [],
                    validate_report_schema(json.loads(input_path.read_text())),
                )
            if input_path.is_file() and output_path.is_file():
                json_text = input_path.read_text(encoding="utf-8")
                markdown = output_path.read_text(encoding="utf-8")
                self.assertEqual([], scan_for_secret_shapes(json_text))
                self.assertEqual([], scan_for_secret_shapes(markdown))


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
    parser.add_argument(
        "--expected-git-ref",
        help="Trusted single-line git ref for this run (e.g. branch name)",
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
        "--input-failure-category",
        default=FAILURE_RUNNER_OUTPUT_MISSING,
        help="Fixed categorical label when --input is missing or unusable",
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
    required = (
        args.expected_git_sha,
        args.expected_manifest_sha256,
        args.expected_git_ref,
        args.ci_run_url,
    )
    if not all(required):
        parser.error(
            "--expected-git-sha, --expected-manifest-sha256, "
            "--expected-git-ref, and --ci-run-url are required"
        )

    try:
        _validate_trusted_hex("expected git sha", args.expected_git_sha, GIT_SHA_HEX, 40)
        _validate_trusted_hex(
            "expected manifest sha256",
            args.expected_manifest_sha256,
            SHA256_HEX,
            64,
        )
        validate_git_ref(args.expected_git_ref)
        validate_failure_message(args.input_failure_category)
        context = RenderContext(
            expected_git_sha=args.expected_git_sha,
            expected_manifest_sha256=args.expected_manifest_sha256,
            expected_git_ref=args.expected_git_ref,
            ci_run_url=args.ci_run_url,
            runner_exit_code=args.runner_exit_code,
            teardown_exit_code=args.teardown_exit_code,
        )
        verdict = render_file(
            input_path=args.input.resolve(),
            output_path=args.output.resolve(),
            context=context,
            input_failure_category=args.input_failure_category,
        )
    except (OSError, ValueError) as exc:
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
