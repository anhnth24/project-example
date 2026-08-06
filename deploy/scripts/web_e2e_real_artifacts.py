#!/usr/bin/env python3
"""Sanitized manifest writer/validator for real web E2E artifacts (P2-20)."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
from pathlib import Path
from typing import Any

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

import redact_secrets  # noqa: E402

SCHEMA_VERSION = 1
MANIFEST_NAME = "manifest.json"
RESULTS_NAME = "playwright-results.json"

# Content markers that must never be retained in staged artifacts.
CONTENT_CANARY_RES = (
    re.compile(rb"E2E_CONTENT_CANARY_DO_NOT_RETAIN"),
    re.compile(rb"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----"),
    re.compile(rb"\bAKIA[0-9A-Z]{16}\b"),
    re.compile(rb"\b(?:ghp|github_pat)_[A-Za-z0-9_]{20,}\b"),
    re.compile(rb"\bpostgres(?:ql)?://[^/\s:@]+:[^@\s/]+@"),
)

REQUIRED_MANIFEST_FIELDS = (
    "schemaVersion",
    "runId",
    "gitSha",
    "gitRef",
    "toolVersions",
    "fixtureChecksum",
    "scenarios",
    "skippedCount",
    "outcomes",
    "durations",
    "teardownResult",
    "artifactChecksums",
)


class ArtifactError(RuntimeError):
    """Fail-closed artifact contract error (message must not contain secrets)."""


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _load_json(path: Path) -> dict[str, Any]:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ArtifactError(f"failed to load JSON: {path.name}") from error
    if not isinstance(payload, dict):
        raise ArtifactError(f"JSON root must be an object: {path.name}")
    return payload


def _tool_version(command: list[str]) -> str:
    try:
        completed = subprocess.run(
            command,
            check=False,
            capture_output=True,
            text=True,
            timeout=5,
        )
    except (OSError, subprocess.TimeoutExpired):
        return "unknown"
    if completed.returncode != 0:
        return "unknown"
    line = (completed.stdout or completed.stderr or "").strip().splitlines()
    return line[0] if line else "unknown"


def _git_value(*args: str) -> str:
    try:
        completed = subprocess.run(
            ["git", *args],
            check=False,
            capture_output=True,
            text=True,
            timeout=5,
        )
    except (OSError, subprocess.TimeoutExpired):
        return "unknown"
    if completed.returncode != 0:
        return "unknown"
    return (completed.stdout or "").strip() or "unknown"


def _iter_playwright_tests(payload: dict[str, Any]) -> list[dict[str, Any]]:
    if isinstance(payload.get("tests"), list) and payload["tests"]:
        return [item for item in payload["tests"] if isinstance(item, dict)]

    collected: list[dict[str, Any]] = []

    def walk_suite(suite: dict[str, Any]) -> None:
        for spec in suite.get("specs", []) if isinstance(suite.get("specs"), list) else []:
            if not isinstance(spec, dict):
                continue
            title = str(spec.get("title") or spec.get("file") or "unnamed")
            tests = spec.get("tests") if isinstance(spec.get("tests"), list) else []
            if not tests:
                collected.append(
                    {
                        "title": title,
                        "outcome": "unknown",
                        "ok": False,
                        "duration": 0,
                    }
                )
                continue
            for test in tests:
                if not isinstance(test, dict):
                    continue
                results = test.get("results") if isinstance(test.get("results"), list) else []
                result = results[-1] if results and isinstance(results[-1], dict) else {}
                status = str(result.get("status") or test.get("status") or "unknown")
                duration = int(result.get("duration") or test.get("duration") or 0)
                ok = status in {"passed", "expected"}
                outcome = status
                if status == "passed":
                    outcome = "expected"
                collected.append(
                    {
                        "title": title,
                        "outcome": outcome,
                        "ok": ok,
                        "duration": duration,
                    }
                )
        for child in suite.get("suites", []) if isinstance(suite.get("suites"), list) else []:
            if isinstance(child, dict):
                walk_suite(child)

    suites = payload.get("suites")
    if isinstance(suites, list):
        for suite in suites:
            if isinstance(suite, dict):
                walk_suite(suite)
    return collected


def _normalize_scenarios(results: dict[str, Any]) -> list[dict[str, Any]]:
    scenarios: list[dict[str, Any]] = []
    for item in _iter_playwright_tests(results):
        title = str(item.get("title") or "unnamed")
        outcome = str(item.get("outcome") or "unknown")
        duration = int(item.get("duration") or 0)
        ok = bool(item.get("ok"))
        if outcome in {"skipped", "interrupted"}:
            ok = False
        scenarios.append(
            {
                "title": title,
                "outcome": outcome,
                "durationMs": duration,
                "ok": ok,
            }
        )
    return scenarios


def _outcomes(scenarios: list[dict[str, Any]]) -> dict[str, int]:
    counts = {"passed": 0, "failed": 0, "skipped": 0, "other": 0}
    for scenario in scenarios:
        outcome = scenario["outcome"]
        if outcome in {"expected", "passed"} and scenario.get("ok"):
            counts["passed"] += 1
        elif outcome == "skipped":
            counts["skipped"] += 1
        elif outcome in {"unexpected", "failed", "timedOut"} or not scenario.get("ok"):
            counts["failed"] += 1
        else:
            counts["other"] += 1
    return counts


def _playwright_version() -> str:
    # Prefer a lightweight probe that cannot be mistaken for `playwright test`.
    for command in (
        ["pnpm", "exec", "playwright", "--version"],
        ["playwright", "--version"],
    ):
        version = _tool_version(command)
        if version != "unknown":
            return version
    return "unknown"


def write_manifest(
    *,
    results_path: Path,
    fixture_path: Path,
    out_path: Path,
    teardown_result: str = "pending",
    environ: dict[str, str] | None = None,
) -> dict[str, Any]:
    env = environ if environ is not None else dict(os.environ)
    if not results_path.is_file():
        raise ArtifactError("missing playwright results")
    if not fixture_path.is_file():
        raise ArtifactError("missing fixture manifest")

    results = _load_json(results_path)
    fixture = _load_json(fixture_path)
    scenarios = _normalize_scenarios(results)
    if not scenarios:
        # Fall back to stats-only payloads from hermetic shims.
        stats = results.get("stats") if isinstance(results.get("stats"), dict) else {}
        skipped = int(stats.get("skipped") or 0)
        expected = int(stats.get("expected") or 0)
        unexpected = int(stats.get("unexpected") or 0)
        if skipped or expected or unexpected:
            for _ in range(expected):
                scenarios.append(
                    {
                        "title": "expected",
                        "outcome": "expected",
                        "durationMs": 0,
                        "ok": True,
                    }
                )
            for _ in range(unexpected):
                scenarios.append(
                    {
                        "title": "unexpected",
                        "outcome": "unexpected",
                        "durationMs": 0,
                        "ok": False,
                    }
                )
            for _ in range(skipped):
                scenarios.append(
                    {
                        "title": "skipped",
                        "outcome": "skipped",
                        "durationMs": 0,
                        "ok": False,
                    }
                )
    if not scenarios:
        raise ArtifactError("playwright results contain no scenarios")

    outcomes = _outcomes(scenarios)
    skipped_count = outcomes["skipped"]
    total_ms = sum(int(item["durationMs"]) for item in scenarios)
    fixture_checksum = str(fixture.get("checksum") or "")
    if not fixture_checksum:
        raise ArtifactError("fixture manifest missing checksum")

    run_id = str(
        env.get("WEB_E2E_REAL_RUN_ID")
        or env.get("MARKHAND_E2E_REAL_RUN_ID")
        or fixture.get("runId")
        or "unknown"
    )

    artifact_dir = out_path.parent
    artifact_checksums: dict[str, str] = {}
    for staged in (results_path, fixture_path):
        resolved = staged.resolve()
        try:
            relative = resolved.relative_to(artifact_dir.resolve()).as_posix()
        except ValueError:
            continue
        artifact_checksums[relative] = _sha256_file(staged)
    if RESULTS_NAME not in artifact_checksums:
        # Results must be staged under the artifact directory for validation.
        raise ArtifactError("playwright results must live under the artifact directory")

    manifest: dict[str, Any] = {
        "schemaVersion": SCHEMA_VERSION,
        "runId": run_id,
        "gitSha": env.get("GITHUB_SHA") or _git_value("rev-parse", "HEAD"),
        "gitRef": env.get("GITHUB_REF")
        or _git_value("rev-parse", "--abbrev-ref", "HEAD"),
        "toolVersions": {
            "node": _tool_version(["node", "--version"]),
            "pnpm": _tool_version(["pnpm", "--version"]),
            "playwright": env.get("WEB_E2E_REAL_PLAYWRIGHT_VERSION") or _playwright_version(),
        },
        "fixtureChecksum": fixture_checksum,
        "scenarios": scenarios,
        "skippedCount": skipped_count,
        "outcomes": outcomes,
        "durations": {"totalMs": total_ms},
        "teardownResult": teardown_result,
        "artifactChecksums": artifact_checksums,
    }
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return manifest


def _scan_path_for_canaries(path: Path) -> list[str]:
    findings: list[str] = []
    try:
        raw = path.read_bytes()
    except OSError as error:
        raise ArtifactError(f"failed to read staged artifact: {path.name}") from error

    for pattern in CONTENT_CANARY_RES:
        if pattern.search(raw):
            findings.append(f"content_canary:{path.name}")
            break

    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError:
        text = raw.decode("utf-8", errors="replace")

    secret_findings = redact_secrets.broad_secret_scan(text)
    for label in secret_findings:
        findings.append(f"secret_canary:{label}:{path.name}")
    return findings


def validate_manifest(*, manifest_path: Path, artifact_dir: Path) -> dict[str, Any]:
    if not manifest_path.is_file():
        raise ArtifactError("missing sanitized manifest")
    if not artifact_dir.is_dir():
        raise ArtifactError("missing artifact directory")

    manifest = _load_json(manifest_path)
    for field in REQUIRED_MANIFEST_FIELDS:
        if field not in manifest:
            raise ArtifactError(f"manifest missing field: {field}")

    if manifest.get("schemaVersion") != SCHEMA_VERSION:
        raise ArtifactError("unsupported manifest schemaVersion")

    skipped = int(manifest.get("skippedCount") or 0)
    if skipped != 0:
        raise ArtifactError("required scenarios were skipped")

    scenarios = manifest.get("scenarios")
    if not isinstance(scenarios, list) or not scenarios:
        raise ArtifactError("manifest scenarios missing")
    for scenario in scenarios:
        if not isinstance(scenario, dict):
            raise ArtifactError("invalid scenario entry")
        outcome = str(scenario.get("outcome") or "")
        if outcome == "skipped":
            raise ArtifactError("required scenarios were skipped")

    checksums = manifest.get("artifactChecksums")
    if not isinstance(checksums, dict) or not checksums:
        raise ArtifactError("manifest artifactChecksums missing")

    results_path = artifact_dir / RESULTS_NAME
    if not results_path.is_file():
        raise ArtifactError("missing playwright results")

    for relative, expected in checksums.items():
        if not isinstance(relative, str) or not isinstance(expected, str):
            raise ArtifactError("invalid artifact checksum entry")
        candidate = artifact_dir / relative
        if not candidate.is_file():
            # Fixture manifest may live beside artifacts; allow basename lookup only
            # inside the artifact dir for staged files.
            raise ArtifactError(f"missing checksummed artifact: {relative}")
        actual = _sha256_file(candidate)
        if actual != expected:
            raise ArtifactError(f"checksum drift: {relative}")

    canary_findings: list[str] = []
    for path in sorted(artifact_dir.rglob("*")):
        if not path.is_file():
            continue
        if path.name.endswith(".credentials.json"):
            raise ArtifactError("credentials file must not be staged in artifact dir")
        canary_findings.extend(_scan_path_for_canaries(path))

    if canary_findings:
        # Labels only — never echo matched secret/content values.
        raise ArtifactError("canary match: " + ",".join(sorted(set(canary_findings))))

    teardown = str(manifest.get("teardownResult") or "")
    if teardown not in {"ok", "pending", "failed"}:
        raise ArtifactError("invalid teardownResult")

    return manifest


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="web_e2e_real_artifacts.py")
    sub = parser.add_subparsers(dest="command", required=True)

    write = sub.add_parser("write")
    write.add_argument("--results", required=True)
    write.add_argument("--fixture", required=True)
    write.add_argument("--out", required=True)
    write.add_argument(
        "--teardown-result",
        default="pending",
        choices=("ok", "pending", "failed"),
    )

    validate = sub.add_parser("validate")
    validate.add_argument("--manifest", required=True)
    validate.add_argument("--artifact-dir", required=True)
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    try:
        args = parser.parse_args(argv)
    except SystemExit as error:
        return int(error.code) if isinstance(error.code, int) else 1

    try:
        if args.command == "write":
            write_manifest(
                results_path=Path(args.results),
                fixture_path=Path(args.fixture),
                out_path=Path(args.out),
                teardown_result=args.teardown_result,
            )
            return 0
        if args.command == "validate":
            validate_manifest(
                manifest_path=Path(args.manifest),
                artifact_dir=Path(args.artifact_dir),
            )
            return 0
    except ArtifactError as error:
        print(f"web_e2e_real_artifacts: {error}", file=sys.stderr)
        return 1
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
