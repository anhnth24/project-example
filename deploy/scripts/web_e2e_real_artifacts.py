#!/usr/bin/env python3
"""Sanitized Playwright result staging and fail-closed artifact validation."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
from pathlib import Path
from typing import Any, Mapping

REQUIRED_MANIFEST_FIELDS = (
    "schemaVersion",
    "runId",
    "git",
    "toolVersions",
    "fixtureChecksum",
    "scenarios",
    "skippedCount",
    "teardown",
    "artifactChecksums",
)

# Exact Playwright titles from
# `MARKHAND_E2E_REAL=1 pnpm --dir web exec playwright test --list`.
REQUIRED_SCENARIO_TITLES: tuple[str, ...] = (
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

# P2-20 retains only the sanitized manifest beside the artifact directory root.
# Reviewed companions may be added here later; until then reject any other file.
ALLOWED_ARTIFACT_COMPANIONS: frozenset[str] = frozenset()

GIT_SHA_RE = re.compile(r"^[0-9a-f]{40}$")
FIXTURE_CHECKSUM_RE = re.compile(r"^[a-f0-9]{64}$")

FORBIDDEN_SCENARIO_KEYS = {
    "errors",
    "stdout",
    "stderr",
    "error",
    "attachments",
    "body",
    "content",
    "password",
    "token",
    "credential",
    "url",
}


class ArtifactError(RuntimeError):
    """Fail-closed artifact helper error with a non-secret message."""


def _sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _sha256_file(path: Path) -> str:
    return _sha256_bytes(path.read_bytes())


def _load_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as error:
        raise ArtifactError(f"missing json file: {path.name}") from error
    except OSError as error:
        raise ArtifactError(f"unreadable json file: {path.name}") from error
    except json.JSONDecodeError as error:
        raise ArtifactError(f"invalid json file: {path.name}") from error


def _run_capture(argv: list[str]) -> str:
    try:
        completed = subprocess.run(
            argv,
            check=False,
            capture_output=True,
            text=True,
            timeout=5,
        )
    except (OSError, subprocess.SubprocessError):
        return ""
    if completed.returncode != 0:
        return ""
    return (completed.stdout or "").strip()


def _git_info(repo_root: Path) -> dict[str, str]:
    sha = _run_capture(["git", "-C", str(repo_root), "rev-parse", "HEAD"])
    ref = _run_capture(
        ["git", "-C", str(repo_root), "rev-parse", "--abbrev-ref", "HEAD"]
    )
    return {
        "sha": sha or "unknown",
        "ref": ref or "unknown",
    }


def _tool_versions(*, repo_root: Path | None = None) -> dict[str, str]:
    versions = {
        "node": _run_capture(["node", "--version"]),
        "pnpm": _run_capture(["pnpm", "--version"]),
        "playwright": "",
    }
    playwright = ""
    if repo_root is not None:
        playwright = _run_capture(
            ["pnpm", "--dir", str(repo_root / "web"), "exec", "playwright", "--version"]
        )
    if not playwright:
        playwright = _run_capture(
            ["pnpm", "exec", "playwright", "--version"]
        ) or _run_capture(["playwright", "--version"])
    versions["playwright"] = playwright
    return versions


def _iter_specs(node: Mapping[str, Any]) -> list[dict[str, Any]]:
    specs: list[dict[str, Any]] = []
    for spec in node.get("specs") or []:
        if isinstance(spec, dict):
            specs.append(spec)
    for child in node.get("suites") or []:
        if isinstance(child, dict):
            specs.extend(_iter_specs(child))
    return specs


def _scenario_outcome(spec: Mapping[str, Any]) -> tuple[str, int]:
    tests = spec.get("tests") or []
    if not isinstance(tests, list) or not tests:
        status = str(spec.get("status") or "")
        if status:
            return status, 0
        return ("passed" if spec.get("ok") else "failed"), 0

    duration = 0
    outcomes: list[str] = []
    for test in tests:
        if not isinstance(test, dict):
            continue
        results = test.get("results") or []
        if isinstance(results, list):
            for result in results:
                if not isinstance(result, dict):
                    continue
                try:
                    duration += int(result.get("duration") or 0)
                except (TypeError, ValueError):
                    pass
                status = str(result.get("status") or "").strip()
                if status:
                    outcomes.append(status)
        status = str(test.get("status") or "").strip()
        if status and not outcomes:
            # Playwright aggregate: expected/unexpected/skipped/flaky
            mapping = {
                "expected": "passed",
                "unexpected": "failed",
                "skipped": "skipped",
                "flaky": "flaky",
            }
            outcomes.append(mapping.get(status, status))
    if not outcomes:
        return ("passed" if spec.get("ok") else "failed"), duration
    if any(item == "failed" for item in outcomes):
        return "failed", duration
    if any(item == "timedOut" for item in outcomes):
        return "timedOut", duration
    if all(item == "skipped" for item in outcomes):
        return "skipped", duration
    if any(item == "flaky" for item in outcomes):
        return "flaky", duration
    return outcomes[-1], duration


def extract_scenarios(results: Mapping[str, Any]) -> tuple[list[dict[str, Any]], int]:
    suites = results.get("suites") or []
    if not isinstance(suites, list):
        raise ArtifactError("playwright results missing suites")
    scenarios: list[dict[str, Any]] = []
    for suite in suites:
        if not isinstance(suite, dict):
            continue
        for spec in _iter_specs(suite):
            title = str(spec.get("title") or "").strip()
            if not title:
                raise ArtifactError("playwright scenario missing title")
            outcome, duration_ms = _scenario_outcome(spec)
            scenario = {
                "title": title,
                "outcome": outcome,
                "durationMs": duration_ms,
            }
            forbidden = set(scenario) & FORBIDDEN_SCENARIO_KEYS
            if forbidden:
                raise ArtifactError("refusing forbidden scenario fields")
            scenarios.append(scenario)
    stats = results.get("stats") if isinstance(results.get("stats"), dict) else {}
    try:
        skipped = int(stats.get("skipped") or 0)
    except (TypeError, ValueError) as error:
        raise ArtifactError("invalid skipped count") from error
    # Also count extracted skipped outcomes if stats omit them.
    skipped = max(skipped, sum(1 for item in scenarios if item["outcome"] == "skipped"))
    return scenarios, skipped


def _fixture_checksum(fixture: Mapping[str, Any]) -> str:
    checksum = fixture.get("checksum")
    if not isinstance(checksum, str) or not FIXTURE_CHECKSUM_RE.fullmatch(checksum):
        raise ArtifactError("fixture checksum missing or invalid")
    return checksum


def _validate_git_metadata(git: Any) -> None:
    if not isinstance(git, dict) or "sha" not in git or "ref" not in git:
        raise ArtifactError("manifest git metadata incomplete")
    sha = git.get("sha")
    ref = git.get("ref")
    if not isinstance(sha, str) or not GIT_SHA_RE.fullmatch(sha) or sha == "unknown":
        raise ArtifactError("manifest git sha invalid")
    if not isinstance(ref, str) or not ref.strip() or ref.strip() == "unknown":
        raise ArtifactError("manifest git ref invalid")


def _validate_tool_versions(tools: Any) -> None:
    if not isinstance(tools, dict):
        raise ArtifactError("manifest toolVersions invalid")
    if not tools:
        raise ArtifactError("manifest toolVersions incomplete")
    for key, value in tools.items():
        if not isinstance(key, str) or not key.strip():
            raise ArtifactError("manifest toolVersions invalid")
        if not isinstance(value, str) or not value.strip():
            raise ArtifactError("manifest toolVersions incomplete")


def _validate_fixture_checksum_value(checksum: Any) -> None:
    if not isinstance(checksum, str) or not FIXTURE_CHECKSUM_RE.fullmatch(checksum):
        raise ArtifactError("fixture checksum missing or invalid")


def _validate_required_scenarios(scenarios: list[Any]) -> None:
    if not scenarios:
        raise ArtifactError("manifest scenarios missing")
    titles: list[str] = []
    for scenario in scenarios:
        if not isinstance(scenario, dict):
            raise ArtifactError("manifest scenario invalid")
        if set(scenario.keys()) - {"title", "outcome", "durationMs"}:
            raise ArtifactError("manifest scenario has extra fields")
        for key in ("title", "outcome", "durationMs"):
            if key not in scenario:
                raise ArtifactError("manifest scenario incomplete")
        title = scenario.get("title")
        outcome = scenario.get("outcome")
        duration = scenario.get("durationMs")
        if not isinstance(title, str) or not title.strip():
            raise ArtifactError("manifest scenario incomplete")
        if not isinstance(outcome, str) or not outcome.strip():
            raise ArtifactError("manifest scenario incomplete")
        if outcome != "passed":
            raise ArtifactError("scenario outcome is not passed")
        if not isinstance(duration, int) or isinstance(duration, bool) or duration < 0:
            raise ArtifactError("manifest scenario duration invalid")
        titles.append(title)

    if len(titles) != len(set(titles)):
        raise ArtifactError("duplicate scenario titles")

    required = set(REQUIRED_SCENARIO_TITLES)
    actual = set(titles)
    missing = required - actual
    if missing:
        raise ArtifactError("missing required scenario")
    unexpected = actual - required
    if unexpected:
        raise ArtifactError("unexpected scenario title")
    if len(titles) != len(REQUIRED_SCENARIO_TITLES):
        raise ArtifactError("required scenario inventory incomplete")


def _atomic_write_json(path: Path, payload: Mapping[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    encoded = json.dumps(payload, indent=2, sort_keys=True) + "\n"
    tmp = path.with_name(f".{path.name}.tmp")
    tmp.write_text(encoded, encoding="utf-8")
    tmp.replace(path)


def _relative_artifact_paths(artifact_dir: Path, manifest_name: str) -> list[str]:
    paths: list[str] = []
    for path in sorted(artifact_dir.rglob("*")):
        if not path.is_file():
            continue
        if path.is_symlink():
            raise ArtifactError("refusing symlinked artifact")
        rel = path.relative_to(artifact_dir).as_posix()
        if rel == manifest_name:
            continue
        if ".." in Path(rel).parts:
            raise ArtifactError("refusing path escape in artifact dir")
        paths.append(rel)
    return paths


def write_manifest(
    *,
    results_path: Path,
    fixture_path: Path,
    out_path: Path,
    teardown: str,
    environ: Mapping[str, str] | None = None,
    repo_root: Path | None = None,
) -> dict[str, Any]:
    env = environ or os.environ
    if teardown not in {"ok", "failed", "pending"}:
        raise ArtifactError("invalid teardown result")
    if not results_path.is_file():
        raise ArtifactError("missing playwright results")
    results = _load_json(results_path)
    if not isinstance(results, dict):
        raise ArtifactError("playwright results must be an object")
    fixture = _load_json(fixture_path)
    if not isinstance(fixture, dict):
        raise ArtifactError("fixture manifest must be an object")

    scenarios, skipped = extract_scenarios(results)
    root = repo_root or Path(__file__).resolve().parents[2]
    manifest: dict[str, Any] = {
        "schemaVersion": 1,
        "runId": str(fixture.get("runId") or env.get("WEB_E2E_REAL_RUN_ID") or ""),
        "git": _git_info(root),
        "toolVersions": _tool_versions(repo_root=root),
        "fixtureChecksum": _fixture_checksum(fixture),
        "scenarios": scenarios,
        "skippedCount": skipped,
        "teardown": {"result": teardown},
        "artifactChecksums": {},
    }
    if not manifest["runId"]:
        raise ArtifactError("run id missing")

    artifact_dir = out_path.parent
    artifact_dir.mkdir(parents=True, exist_ok=True)
    _atomic_write_json(out_path, manifest)

    checksums: dict[str, str] = {}
    for rel in _relative_artifact_paths(artifact_dir, out_path.name):
        if rel not in ALLOWED_ARTIFACT_COMPANIONS:
            # Do not retain arbitrary pre-existing files as companions.
            continue
        checksums[rel] = _sha256_file(artifact_dir / rel)
    # Include the manifest checksum of the payload without recursive self-hash:
    # hash the canonical bytes currently on disk after the first write, then store
    # companion checksums only. Manifest integrity is checked via required fields
    # plus companion checksum map consistency.
    manifest["artifactChecksums"] = checksums
    _atomic_write_json(out_path, manifest)
    return manifest


def _configured_canaries(environ: Mapping[str, str]) -> list[str]:
    values: list[str] = []
    for key in ("WEB_E2E_REAL_SECRET_CANARIES", "WEB_E2E_REAL_CONTENT_CANARIES"):
        raw = environ.get(key, "")
        for part in raw.replace(",", "\n").splitlines():
            item = part.strip()
            if item:
                values.append(item)
    return values


def validate_manifest(
    *,
    manifest_path: Path,
    artifact_dir: Path,
    environ: Mapping[str, str] | None = None,
) -> None:
    env = environ or os.environ
    if not manifest_path.is_file():
        raise ArtifactError("missing manifest")
    if not artifact_dir.is_dir():
        raise ArtifactError("missing artifact dir")
    if manifest_path.is_symlink() or artifact_dir.is_symlink():
        raise ArtifactError("refusing symlinked manifest or artifact dir")

    manifest = _load_json(manifest_path)
    if not isinstance(manifest, dict):
        raise ArtifactError("manifest must be an object")

    for field in REQUIRED_MANIFEST_FIELDS:
        if field not in manifest:
            raise ArtifactError(f"manifest missing field: {field}")

    _validate_git_metadata(manifest.get("git"))
    _validate_tool_versions(manifest.get("toolVersions"))
    _validate_fixture_checksum_value(manifest.get("fixtureChecksum"))

    scenarios = manifest.get("scenarios")
    if not isinstance(scenarios, list):
        raise ArtifactError("manifest scenarios missing")
    _validate_required_scenarios(scenarios)

    try:
        skipped = int(manifest.get("skippedCount"))
    except (TypeError, ValueError) as error:
        raise ArtifactError("manifest skippedCount invalid") from error
    if skipped != 0:
        raise ArtifactError("required scenarios were skipped")

    teardown = manifest.get("teardown")
    if not isinstance(teardown, dict) or teardown.get("result") != "ok":
        raise ArtifactError("teardown result is not ok")

    checksums = manifest.get("artifactChecksums")
    if not isinstance(checksums, dict):
        raise ArtifactError("artifactChecksums invalid")

    expected = {
        rel: str(digest)
        for rel, digest in checksums.items()
        if isinstance(rel, str) and isinstance(digest, str)
    }
    if len(expected) != len(checksums):
        raise ArtifactError("artifactChecksums contains invalid entries")
    if any(rel not in ALLOWED_ARTIFACT_COMPANIONS for rel in expected):
        raise ArtifactError("unallowlisted artifact companion in checksums")

    actual_files = set(_relative_artifact_paths(artifact_dir, manifest_path.name))
    if any(rel not in ALLOWED_ARTIFACT_COMPANIONS for rel in actual_files):
        raise ArtifactError("unallowlisted artifact companion")
    expected_files = set(expected)
    if actual_files != expected_files:
        raise ArtifactError("artifact inventory mismatch")

    for rel, digest in sorted(expected.items()):
        path = artifact_dir / rel
        if not path.is_file() or path.is_symlink():
            raise ArtifactError("artifact missing or symlinked")
        if _sha256_file(path) != digest:
            raise ArtifactError("artifact checksum mismatch")

    canaries = _configured_canaries(env)
    if canaries:
        for path in sorted(artifact_dir.rglob("*")):
            if not path.is_file():
                continue
            if path.is_symlink():
                raise ArtifactError("refusing symlinked artifact during canary scan")
            try:
                text = path.read_text(encoding="utf-8", errors="replace")
            except OSError as error:
                raise ArtifactError("unreadable artifact during canary scan") from error
            for canary in canaries:
                if canary and canary in text:
                    raise ArtifactError("secret or content canary matched")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="web_e2e_real_artifacts.py")
    sub = parser.add_subparsers(dest="command", required=True)

    write = sub.add_parser("write")
    write.add_argument("--results", required=True)
    write.add_argument("--fixture", required=True)
    write.add_argument("--out", required=True)
    write.add_argument("--teardown", required=True)

    validate = sub.add_parser("validate")
    validate.add_argument("--manifest", required=True)
    validate.add_argument("--artifact-dir", required=True)
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        if args.command == "write":
            write_manifest(
                results_path=Path(args.results),
                fixture_path=Path(args.fixture),
                out_path=Path(args.out),
                teardown=str(args.teardown),
            )
            return 0
        validate_manifest(
            manifest_path=Path(args.manifest),
            artifact_dir=Path(args.artifact_dir),
        )
        return 0
    except ArtifactError as error:
        print(f"web_e2e_real_artifacts: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
