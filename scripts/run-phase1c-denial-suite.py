#!/usr/bin/env python3
"""Phase 1C connected multi-org denial manifest runner.

Reads the denial manifest, validates executable rows against integration test
sources, groups rows by binary, runs each unique binary once via cargo test with
``--include-ignored``, scans bounded child output for foreign-marker leakage and
secret-shaped material, and writes a sanitized deterministic JSON report.

Hermetic ``--self-test`` uses temporary manifests/sources and a fake executor;
it never runs cargo, network, or real repo integration tests.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
import unittest
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Mapping, Protocol, Sequence

REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_MANIFEST = REPO_ROOT / "crates/server/tests/fixtures/multi-org-denial.manifest.json"
SERVER_TESTS_ROOT = REPO_ROOT / "crates/server/tests"
CARGO_PACKAGE = "fileconv-server"
REQUIRED_ENV_VAR = "MARKHAND_TEST_REQUIRED"
REQUIRED_ENV_VALUE = "1"
MAX_CAPTURE_BYTES = 256 * 1024
REPORT_SCHEMA_VERSION = 1

BINARY_ID_RE = re.compile(r"^[a-z][a-z0-9_]*$")
ALLOWED_STATUSES = frozenset({"executable", "na", "deferred"})
RUST_TEST_FN_RE = re.compile(
    r"^\s*(?:pub\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)",
    re.MULTILINE,
)

# Foreign-marker categories aligned with crates/server/tests/common/multi_org_denial.rs.
FOREIGN_MARKER_CATEGORIES = (
    "org_id",
    "user_id",
    "collection_id",
    "document_id",
    "version_id",
    "chunk_id",
    "job_id",
    "conflict_id",
    "object_key",
    "name",
    "marker_string",
)

# Redaction patterns — labels only in findings; never emit matched secret values.
BEARER_RE = re.compile(r"(?i)\bBearer\s+[A-Za-z0-9._\-+=/]{8,}")
JWT_RE = re.compile(
    r"\beyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\b"
)
ASSIGN_SECRET_RE = re.compile(
    r"(?i)\b(password|passwd|secret|token|api[_-]?key|access[_-]?key|private[_-]?key)\s*[=:]\s*"
    r"([^\s'\"\\]+|'[^']*'|\"[^\"]*\")"
)
DB_URL_RE = re.compile(
    r"(?i)\b(postgres(?:ql)?|mysql|mongodb|redis)://[^\s'\"]+"
)
CI_SECRET_RE = re.compile(
    r"(?i)\b(GITHUB_TOKEN|AWS_SECRET_ACCESS_KEY|NPM_TOKEN|PYPI_API_TOKEN)\s*[=:]\s*\S+"
)


class RunnerNotImplemented(RuntimeError):
    """Raised for RED-stubbed execution/redaction/report paths."""


@dataclass(frozen=True)
class ManifestRow:
    id: str
    status: str
    guard_inventory_ref: str
    layer: str
    binary: str | None = None
    test_name: str | None = None
    operation_id: str | None = None
    na_category: str | None = None
    coverage_state: str | None = None
    deferred_task: str | None = None

    @classmethod
    def from_dict(cls, raw: Mapping[str, Any]) -> ManifestRow:
        return cls(
            id=str(raw["id"]),
            status=str(raw["status"]),
            guard_inventory_ref=str(raw["guardInventoryRef"]),
            layer=str(raw.get("layer", "")),
            binary=raw.get("binary"),
            test_name=raw.get("testName"),
            operation_id=raw.get("operationId"),
            na_category=raw.get("naCategory"),
            coverage_state=raw.get("coverageState"),
            deferred_task=raw.get("deferredTask"),
        )


@dataclass(frozen=True)
class DenialManifest:
    version: int
    rows: tuple[ManifestRow, ...]

    @classmethod
    def from_dict(cls, raw: Mapping[str, Any]) -> DenialManifest:
        version = int(raw["version"])
        rows = tuple(ManifestRow.from_dict(row) for row in raw["rows"])
        return cls(version=version, rows=rows)


@dataclass(frozen=True)
class DenialFixture:
    version: int
    indexed_markers: dict[str, str]
    duplicate_names: dict[str, Any]
    object_key_template: str

    @classmethod
    def from_dict(cls, raw: Mapping[str, Any]) -> DenialFixture:
        return cls(
            version=int(raw["version"]),
            indexed_markers=dict(raw.get("indexedMarkers") or {}),
            duplicate_names=dict(raw.get("duplicateNames") or {}),
            object_key_template=str(raw.get("objectKeyTemplate") or ""),
        )


@dataclass(frozen=True)
class ForeignNeedle:
    category: str
    value: str

    def label(self) -> str:
        return f"{self.category}:{_short_hash(self.value)}"


@dataclass(frozen=True)
class LeakFinding:
    category: str
    label: str
    hash: str

    def as_dict(self) -> dict[str, str]:
        return {"category": self.category, "label": self.label, "hash": self.hash}


@dataclass(frozen=True)
class ChildResult:
    binary: str
    exit_code: int
    stdout: str
    stderr: str


@dataclass
class RunReport:
    schema_version: int = REPORT_SCHEMA_VERSION
    git_sha_full: str = ""
    manifest_sha256: str = ""
    executable_count: int = 0
    na_count: int = 0
    deferred_count: int = 0
    binaries_run: list[str] = field(default_factory=list)
    failures: list[str] = field(default_factory=list)
    leakage_count: int = 0
    findings: list[LeakFinding] = field(default_factory=list)
    redaction_scan: dict[str, Any] = field(
        default_factory=lambda: {"passed": False, "findings": []}
    )


class CommandExecutor(Protocol):
    def run(
        self,
        command: Sequence[str],
        *,
        cwd: Path,
        env: Mapping[str, str],
    ) -> ChildResult: ...


class SubprocessExecutor:
    """Real cargo executor — RED: not wired into ``run_suite`` yet."""

    def run(
        self,
        command: Sequence[str],
        *,
        cwd: Path,
        env: Mapping[str, str],
    ) -> ChildResult:
        raise RunnerNotImplemented("subprocess execution is not implemented in RED")


@dataclass(frozen=True)
class FakeExecutorOutcome:
    exit_code: int = 0
    stdout: str = ""
    stderr: str = ""


class FakeExecutor:
    """Hermetic executor for self-tests."""

    def __init__(self, outcomes: Mapping[str, FakeExecutorOutcome] | None = None) -> None:
        self.outcomes = dict(outcomes or {})
        self.calls: list[list[str]] = []

    def run(
        self,
        command: Sequence[str],
        *,
        cwd: Path,
        env: Mapping[str, str],
    ) -> ChildResult:
        self.calls.append(list(command))
        binary = _command_binary(command)
        outcome = self.outcomes.get(binary, FakeExecutorOutcome())
        return ChildResult(
            binary=binary,
            exit_code=outcome.exit_code,
            stdout=outcome.stdout,
            stderr=outcome.stderr,
        )


def _short_hash(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()[:12]


def _sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _sha256_file(path: Path) -> str:
    return _sha256_bytes(path.read_bytes())


def _command_binary(command: Sequence[str]) -> str:
    args = list(command)
    try:
        test_index = args.index("--test")
        return args[test_index + 1]
    except (ValueError, IndexError) as exc:
        raise ValueError(f"command missing --test binary: {args!r}") from exc


def required_env_satisfied(env: Mapping[str, str] | None = None) -> bool:
    mapping = env if env is not None else os.environ
    return mapping.get(REQUIRED_ENV_VAR) == REQUIRED_ENV_VALUE


def validate_required_env(env: Mapping[str, str] | None = None) -> None:
    if not required_env_satisfied(env):
        raise ValueError(
            f"{REQUIRED_ENV_VAR}={REQUIRED_ENV_VALUE!r} is required for denial suite execution"
        )


def is_safe_binary_identifier(binary: str) -> bool:
    return bool(BINARY_ID_RE.fullmatch(binary))


def load_manifest(path: Path) -> DenialManifest:
    raw = json.loads(path.read_text(encoding="utf-8"))
    return DenialManifest.from_dict(raw)


def load_fixture(path: Path) -> DenialFixture:
    raw = json.loads(path.read_text(encoding="utf-8"))
    return DenialFixture.from_dict(raw)


def fixture_path_for_manifest(manifest_path: Path) -> Path:
    return manifest_path.with_name("multi-org-denial.fixture.json")


def validate_manifest_schema(manifest: DenialManifest) -> list[str]:
    errors: list[str] = []
    if manifest.version < 1:
        errors.append(f"manifest version must be >= 1, got {manifest.version}")
    if not manifest.rows:
        errors.append("manifest rows must not be empty")

    seen_ids: set[str] = set()
    for row in manifest.rows:
        if row.status not in ALLOWED_STATUSES:
            errors.append(f"manifest row {row.id} has unknown status {row.status!r}")
            continue
        if row.status == "deferred":
            errors.append(f"manifest row {row.id} has unresolved deferred status")
        if row.id in seen_ids:
            errors.append(f"duplicate manifest row id {row.id}")
        seen_ids.add(row.id)

        if not row.guard_inventory_ref.strip():
            errors.append(f"manifest row {row.id} missing guardInventoryRef")

        if row.status == "na":
            if not row.na_category:
                errors.append(f"manifest row {row.id} missing naCategory")
            if row.coverage_state == "deferred":
                errors.append(
                    f"manifest row {row.id} cannot combine status=na with coverageState=deferred"
                )
        elif row.status == "executable":
            if row.coverage_state == "deferred":
                errors.append(
                    f"executable manifest row {row.id} cannot declare coverageState=deferred"
                )
            if not row.binary or not row.binary.strip():
                errors.append(f"executable manifest row {row.id} missing binary")
            elif not is_safe_binary_identifier(row.binary):
                errors.append(
                    f"executable manifest row {row.id} binary {row.binary!r} is not a safe "
                    "Rust integration target identifier"
                )
            if not row.test_name or not row.test_name.strip():
                errors.append(f"executable manifest row {row.id} missing testName")
    return errors


def extract_rust_test_names(source: str) -> set[str]:
    return set(RUST_TEST_FN_RE.findall(source))


def integration_test_source_path(tests_root: Path, binary: str) -> Path:
    return tests_root / f"{binary}.rs"


def validate_executable_sources(
    manifest: DenialManifest,
    *,
    tests_root: Path,
) -> list[str]:
    errors: list[str] = []
    cache: dict[str, set[str]] = {}

    for row in manifest.rows:
        if row.status != "executable":
            continue
        assert row.binary is not None
        assert row.test_name is not None

        if not is_safe_binary_identifier(row.binary):
            errors.append(
                f"manifest row {row.id} binary {row.binary!r} is not a safe integration target"
            )
            continue

        source_path = integration_test_source_path(tests_root, row.binary)
        if not source_path.is_file():
            errors.append(
                f"manifest row {row.id} missing integration source at {source_path}"
            )
            continue

        if row.binary not in cache:
            try:
                source = source_path.read_text(encoding="utf-8")
            except OSError as exc:
                errors.append(
                    f"manifest row {row.id} cannot read integration source {source_path}: {exc}"
                )
                continue
            cache[row.binary] = extract_rust_test_names(source)

        if row.test_name not in cache[row.binary]:
            errors.append(
                f"manifest row {row.id} testName {row.test_name!r} not found in {source_path}"
            )
    return errors


def count_rows_by_status(manifest: DenialManifest) -> tuple[int, int, int]:
    executable = sum(1 for row in manifest.rows if row.status == "executable")
    na = sum(1 for row in manifest.rows if row.status == "na")
    deferred = sum(1 for row in manifest.rows if row.status == "deferred")
    return executable, na, deferred


def group_executable_rows_by_binary(manifest: DenialManifest) -> dict[str, list[ManifestRow]]:
    grouped: dict[str, list[ManifestRow]] = {}
    for row in manifest.rows:
        if row.status != "executable":
            continue
        assert row.binary is not None
        grouped.setdefault(row.binary, []).append(row)
    return dict(sorted(grouped.items()))


def build_cargo_command(binary: str) -> list[str]:
    if not is_safe_binary_identifier(binary):
        raise ValueError(f"unsafe integration binary identifier: {binary!r}")
    return [
        "cargo",
        "test",
        "-p",
        CARGO_PACKAGE,
        "--test",
        binary,
        "--",
        "--include-ignored",
        "--nocapture",
    ]


def resolve_git_sha_full(repo_root: Path) -> str:
    try:
        completed = subprocess.run(
            ["git", "-C", str(repo_root), "rev-parse", "HEAD"],
            check=True,
            capture_output=True,
            text=True,
        )
    except (OSError, subprocess.CalledProcessError):
        return "0" * 40
    return completed.stdout.strip()


def static_foreign_needles(fixture: DenialFixture) -> list[ForeignNeedle]:
    needles: list[ForeignNeedle] = []
    for value in fixture.indexed_markers.values():
        needles.append(ForeignNeedle("marker_string", value))
    document = str(fixture.duplicate_names.get("document") or "")
    if document:
        needles.append(ForeignNeedle("name", document))
    collections_by_visibility = fixture.duplicate_names.get("collectionsByVisibility") or {}
    if isinstance(collections_by_visibility, dict):
        for value in collections_by_visibility.values():
            needles.append(ForeignNeedle("name", str(value)))
    private_name = collections_by_visibility.get("private") if isinstance(
        collections_by_visibility, dict
    ) else None
    org_name = str(fixture.duplicate_names.get("collection") or "")
    if org_name and org_name != str(private_name or ""):
        needles.append(ForeignNeedle("name", org_name))
    if fixture.object_key_template:
        needles.append(ForeignNeedle("object_key", fixture.object_key_template))
    return needles


def extract_runtime_foreign_needles(text: str) -> list[ForeignNeedle]:
    """RED: runtime marker extraction from child output is not implemented."""
    raise RunnerNotImplemented("runtime foreign-marker extraction is not implemented in RED")


def collect_foreign_needles(fixture: DenialFixture, child_output: str) -> list[ForeignNeedle]:
    needles = static_foreign_needles(fixture)
    needles.extend(extract_runtime_foreign_needles(child_output))
    return needles


def scan_for_foreign_markers(
    text: str,
    needles: Sequence[ForeignNeedle],
) -> list[LeakFinding]:
    findings: list[LeakFinding] = []
    lowered = text.lower()
    for needle in needles:
        variants = {needle.value, needle.value.lower(), needle.value.upper()}
        if any(variant and variant.lower() in lowered for variant in variants):
            digest = hashlib.sha256(needle.value.encode("utf-8")).hexdigest()
            findings.append(
                LeakFinding(
                    category="foreign_marker",
                    label=needle.label(),
                    hash=digest,
                )
            )
    return findings


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
    return labels


def redact_text(text: str) -> str:
    """RED: deterministic secret redaction is not implemented."""
    raise RunnerNotImplemented("secret redaction is not implemented in RED")


def redact_report_dict(report: dict[str, Any]) -> dict[str, Any]:
    """RED: recursive report redaction is not implemented."""
    raise RunnerNotImplemented("report redaction is not implemented in RED")


def bound_capture(text: str, *, limit: int = MAX_CAPTURE_BYTES) -> str:
    encoded = text.encode("utf-8", errors="replace")
    if len(encoded) <= limit:
        return text
    truncated = encoded[:limit].decode("utf-8", errors="ignore")
    return truncated + "\n<output truncated>"


def build_deterministic_report_dict(report: RunReport) -> dict[str, Any]:
    """RED: canonical deterministic report assembly is not implemented."""
    raise RunnerNotImplemented("deterministic report assembly is not implemented in RED")


def write_report_atomic(path: Path, report_dict: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = json.dumps(report_dict, indent=2, sort_keys=True) + "\n"
    with tempfile.NamedTemporaryFile(
        "w",
        encoding="utf-8",
        dir=path.parent,
        delete=False,
    ) as handle:
        handle.write(payload)
        temp_name = handle.name
    os.replace(temp_name, path)


def execute_grouped_binaries(
    binaries: Sequence[str],
    *,
    executor: CommandExecutor,
    cwd: Path,
    env: Mapping[str, str],
) -> list[ChildResult]:
    """RED: grouped binary execution is not implemented."""
    raise RunnerNotImplemented("grouped binary execution is not implemented in RED")


def assemble_report(
    *,
    manifest: DenialManifest,
    manifest_path: Path,
    fixture: DenialFixture,
    child_results: Sequence[ChildResult],
    validation_errors: Sequence[str],
    repo_root: Path,
) -> RunReport:
    """RED: full report assembly with leakage scanning is not implemented."""
    raise RunnerNotImplemented("report assembly is not implemented in RED")


def run_suite(
    *,
    manifest_path: Path,
    output_path: Path | None,
    repo_root: Path,
    tests_root: Path,
    executor: CommandExecutor,
    env: Mapping[str, str] | None = None,
    git_sha_resolver: Callable[[Path], str] = resolve_git_sha_full,
) -> tuple[int, RunReport | None]:
    runtime_env = dict(env if env is not None else os.environ)
    report = RunReport(
        git_sha_full=git_sha_resolver(repo_root),
        manifest_sha256=_sha256_file(manifest_path),
    )

    try:
        validate_required_env(runtime_env)
    except ValueError as exc:
        report.failures.append(str(exc))
        exit_code = 2
        if output_path is not None:
            try:
                payload = build_deterministic_report_dict(report)
                write_report_atomic(output_path, redact_report_dict(payload))
            except RunnerNotImplemented:
                pass
        return exit_code, report

    try:
        manifest = load_manifest(manifest_path)
    except (OSError, json.JSONDecodeError, KeyError, TypeError, ValueError) as exc:
        report.failures.append(f"manifest load failed: {exc}")
        exit_code = 2
        if output_path is not None:
            try:
                payload = build_deterministic_report_dict(report)
                write_report_atomic(output_path, redact_report_dict(payload))
            except RunnerNotImplemented:
                pass
        return exit_code, report

    executable_count, na_count, deferred_count = count_rows_by_status(manifest)
    report.executable_count = executable_count
    report.na_count = na_count
    report.deferred_count = deferred_count

    validation_errors = validate_manifest_schema(manifest)
    validation_errors.extend(
        validate_executable_sources(manifest, tests_root=tests_root)
    )
    if validation_errors:
        report.failures.extend(sorted(validation_errors))
        exit_code = 2
        if output_path is not None:
            try:
                payload = build_deterministic_report_dict(report)
                write_report_atomic(output_path, redact_report_dict(payload))
            except RunnerNotImplemented:
                pass
        return exit_code, report

    fixture_path = fixture_path_for_manifest(manifest_path)
    try:
        fixture = load_fixture(fixture_path)
    except (OSError, json.JSONDecodeError, KeyError, TypeError, ValueError) as exc:
        report.failures.append(f"fixture load failed: {exc}")
        exit_code = 2
        if output_path is not None:
            try:
                payload = build_deterministic_report_dict(report)
                write_report_atomic(output_path, redact_report_dict(payload))
            except RunnerNotImplemented:
                pass
        return exit_code, report

    grouped = group_executable_rows_by_binary(manifest)
    try:
        child_results = execute_grouped_binaries(
            list(grouped.keys()),
            executor=executor,
            cwd=repo_root,
            env=runtime_env,
        )
        report = assemble_report(
            manifest=manifest,
            manifest_path=manifest_path,
            fixture=fixture,
            child_results=child_results,
            validation_errors=[],
            repo_root=repo_root,
        )
    except RunnerNotImplemented as exc:
        report.failures.append(str(exc))
        exit_code = 1
        if output_path is not None:
            try:
                payload = build_deterministic_report_dict(report)
                write_report_atomic(output_path, redact_report_dict(payload))
            except RunnerNotImplemented:
                pass
        return exit_code, report

    exit_code = 0
    if report.failures or report.leakage_count > 0:
        exit_code = 1
    if output_path is not None:
        payload = build_deterministic_report_dict(report)
        write_report_atomic(output_path, redact_report_dict(payload))
    return exit_code, report


# ---------------------------------------------------------------------------
# Hermetic self-tests
# ---------------------------------------------------------------------------


class DenialRunnerSelfTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.root = Path(self.temp_dir.name)
        self.tests_root = self.root / "tests"
        self.tests_root.mkdir(parents=True)
        self.fixture_path = self.root / "multi-org-denial.fixture.json"
        self.fixture_path.write_text(
            json.dumps(
                {
                    "version": 1,
                    "indexedMarkers": {
                        "orgAlpha": "phase1c-marker-alpha",
                        "orgBeta": "phase1c-marker-beta",
                    },
                    "duplicateNames": {
                        "collection": "Shared Contract Collection",
                        "document": "Shared Contract Document",
                        "collectionsByVisibility": {
                            "private": "Shared Private Collection",
                            "org": "Shared Contract Collection",
                            "groups": "Shared Groups Collection",
                        },
                    },
                    "objectKeyTemplate": "denial/{orgKey}/{marker}.txt",
                }
            ),
            encoding="utf-8",
        )
        self.required_env = {REQUIRED_ENV_VAR: REQUIRED_ENV_VALUE}

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def write_manifest(self, rows: list[dict[str, Any]]) -> Path:
        manifest_path = self.root / "manifest.json"
        manifest_path.write_text(
            json.dumps({"version": 1, "rows": rows}, indent=2),
            encoding="utf-8",
        )
        return manifest_path

    def write_source(self, binary: str, test_names: Sequence[str]) -> None:
        body = "\n".join(f"async fn {name}() {{}}" for name in test_names)
        (self.tests_root / f"{binary}.rs").write_text(body + "\n", encoding="utf-8")

    def test_unknown_binary_rejected_before_execution(self) -> None:
        manifest_path = self.write_manifest(
            [
                {
                    "id": "denial-bad",
                    "binary": "not/a/binary",
                    "testName": "sample_test",
                    "operationId": "ask",
                    "guardInventoryRef": "ask",
                    "layer": "http",
                    "status": "executable",
                }
            ]
        )
        manifest = load_manifest(manifest_path)
        errors = validate_manifest_schema(manifest)
        errors.extend(validate_executable_sources(manifest, tests_root=self.tests_root))
        self.assertTrue(any("safe" in error for error in errors))

    def test_missing_test_source_rejected_before_execution(self) -> None:
        manifest_path = self.write_manifest(
            [
                {
                    "id": "denial-missing",
                    "binary": "missing_binary",
                    "testName": "sample_test",
                    "operationId": "ask",
                    "guardInventoryRef": "ask",
                    "layer": "http",
                    "status": "executable",
                }
            ]
        )
        manifest = load_manifest(manifest_path)
        errors = validate_executable_sources(manifest, tests_root=self.tests_root)
        self.assertTrue(any("missing integration source" in error for error in errors))

    def test_missing_test_name_rejected_before_execution(self) -> None:
        self.write_source("api_http_contracts", ["live_http_retrieval_refuses_foreign_collection_scope"])
        manifest_path = self.write_manifest(
            [
                {
                    "id": "denial-missing-name",
                    "binary": "api_http_contracts",
                    "testName": "definitely_missing_test_name",
                    "operationId": "ask",
                    "guardInventoryRef": "ask",
                    "layer": "http",
                    "status": "executable",
                }
            ]
        )
        manifest = load_manifest(manifest_path)
        errors = validate_executable_sources(manifest, tests_root=self.tests_root)
        self.assertTrue(any("testName" in error for error in errors))

    def test_missing_required_env_fails_closed(self) -> None:
        manifest_path = self.write_manifest([])
        exit_code, report = run_suite(
            manifest_path=manifest_path,
            output_path=self.root / "report.json",
            repo_root=self.root,
            tests_root=self.tests_root,
            executor=FakeExecutor(),
            env={},
            git_sha_resolver=lambda _root: "a" * 40,
        )
        self.assertNotEqual(exit_code, 0)
        self.assertIsNotNone(report)
        assert report is not None
        self.assertTrue(any(REQUIRED_ENV_VAR in failure for failure in report.failures))

    def test_nonzero_child_exit_is_failure(self) -> None:
        self.write_source("api_http_contracts", ["live_http_retrieval_refuses_foreign_collection_scope"])
        manifest_path = self.write_manifest(
            [
                {
                    "id": "denial-ask",
                    "binary": "api_http_contracts",
                    "testName": "live_http_retrieval_refuses_foreign_collection_scope",
                    "operationId": "ask",
                    "guardInventoryRef": "ask",
                    "layer": "http",
                    "status": "executable",
                }
            ]
        )
        executor = FakeExecutor(
            {
                "api_http_contracts": FakeExecutorOutcome(
                    exit_code=101,
                    stdout="test live_http_retrieval_refuses_foreign_collection_scope ... FAILED\n",
                    stderr="assertion failed\n",
                )
            }
        )
        exit_code, report = run_suite(
            manifest_path=manifest_path,
            output_path=self.root / "report.json",
            repo_root=self.root,
            tests_root=self.tests_root,
            executor=executor,
            env=self.required_env,
            git_sha_resolver=lambda _root: "b" * 40,
        )
        self.assertNotEqual(exit_code, 0)
        self.assertIsNotNone(report)
        assert report is not None
        self.assertTrue(
            any("api_http_contracts" in failure or "101" in failure for failure in report.failures),
            report.failures,
        )

    def test_foreign_marker_in_child_output_creates_leak_finding(self) -> None:
        self.write_source("multi_org_denial", ["shared_world_http_surfaces_respect_org_scope"])
        manifest_path = self.write_manifest(
            [
                {
                    "id": "denial-authMe",
                    "binary": "multi_org_denial",
                    "testName": "shared_world_http_surfaces_respect_org_scope",
                    "operationId": "authMe",
                    "guardInventoryRef": "authMe",
                    "layer": "http",
                    "status": "executable",
                }
            ]
        )
        executor = FakeExecutor(
            {
                "multi_org_denial": FakeExecutorOutcome(
                    stdout="leaked foreign marker phase1c-marker-beta in body\n",
                )
            }
        )
        exit_code, report = run_suite(
            manifest_path=manifest_path,
            output_path=self.root / "report.json",
            repo_root=self.root,
            tests_root=self.tests_root,
            executor=executor,
            env=self.required_env,
            git_sha_resolver=lambda _root: "c" * 40,
        )
        self.assertNotEqual(exit_code, 0)
        self.assertIsNotNone(report)
        assert report is not None
        self.assertGreater(report.leakage_count, 0)
        self.assertTrue(report.findings)

    def test_command_grouping_runs_each_binary_once_in_sorted_order(self) -> None:
        self.write_source("alpha_binary", ["alpha_test"])
        self.write_source("zebra_binary", ["zebra_test"])
        manifest_path = self.write_manifest(
            [
                {
                    "id": "denial-z1",
                    "binary": "zebra_binary",
                    "testName": "zebra_test",
                    "operationId": "ask",
                    "guardInventoryRef": "ask",
                    "layer": "http",
                    "status": "executable",
                },
                {
                    "id": "denial-z2",
                    "binary": "zebra_binary",
                    "testName": "zebra_test",
                    "operationId": "askStream",
                    "guardInventoryRef": "askStream",
                    "layer": "http",
                    "status": "executable",
                },
                {
                    "id": "denial-a1",
                    "binary": "alpha_binary",
                    "testName": "alpha_test",
                    "operationId": "authMe",
                    "guardInventoryRef": "authMe",
                    "layer": "http",
                    "status": "executable",
                },
                {
                    "id": "na-export",
                    "guardInventoryRef": "export_route_absent",
                    "layer": "http",
                    "status": "na",
                    "naCategory": "export_route_absent",
                },
            ]
        )
        manifest = load_manifest(manifest_path)
        grouped = group_executable_rows_by_binary(manifest)
        self.assertEqual(list(grouped.keys()), ["alpha_binary", "zebra_binary"])
        self.assertEqual(len(grouped["zebra_binary"]), 2)

        executor = FakeExecutor()
        with self.assertRaises(RunnerNotImplemented):
            execute_grouped_binaries(
                list(grouped.keys()),
                executor=executor,
                cwd=self.root,
                env=self.required_env,
            )

        # Contract: cargo command uses argv list without shell metacharacters.
        command = build_cargo_command("alpha_binary")
        self.assertEqual(
            command,
            [
                "cargo",
                "test",
                "-p",
                CARGO_PACKAGE,
                "--test",
                "alpha_binary",
                "--",
                "--include-ignored",
                "--nocapture",
            ],
        )
        self.assertNotIn(";", " ".join(command))
        self.assertNotIn("|", " ".join(command))

    def test_redaction_removes_canaries_from_serialized_report(self) -> None:
        canary = "Bearer phase1c-selftest-canary-token-value"
        raw_report = {
            "gitShaFull": "d" * 40,
            "manifestSha256": "e" * 64,
            "failures": [f"child stderr mentioned {canary}"],
            "findings": [],
            "leakageCount": 0,
        }
        redacted = redact_report_dict(raw_report)
        serialized = json.dumps(redacted, sort_keys=True)
        self.assertNotIn(canary, serialized)
        self.assertNotIn("phase1c-selftest-canary-token-value", serialized)

    def test_deterministic_json_for_identical_inputs(self) -> None:
        report = RunReport(
            git_sha_full="f" * 40,
            manifest_sha256="1" * 64,
            executable_count=2,
            na_count=1,
            deferred_count=0,
            binaries_run=["beta_binary", "alpha_binary"],
            failures=["zebra failure", "alpha failure"],
            leakage_count=1,
            findings=[
                LeakFinding("foreign_marker", "marker_string:abc", "a" * 64),
            ],
        )
        first = json.dumps(build_deterministic_report_dict(report), sort_keys=True)
        second = json.dumps(build_deterministic_report_dict(report), sort_keys=True)
        self.assertEqual(first, second)
        payload = json.loads(first)
        self.assertEqual(payload["binariesRun"], ["alpha_binary", "beta_binary"])
        self.assertEqual(payload["failures"], ["alpha failure", "zebra failure"])
        self.assertNotIn("generatedAt", payload)
        self.assertNotIn("durationMs", payload)

    def test_atomic_failure_still_writes_sanitized_report(self) -> None:
        manifest_path = self.write_manifest(
            [
                {
                    "id": "denial-bad",
                    "binary": "missing_binary",
                    "testName": "missing_test",
                    "operationId": "ask",
                    "guardInventoryRef": "ask",
                    "layer": "http",
                    "status": "executable",
                }
            ]
        )
        output_path = self.root / "nested" / "manifest-run.json"
        exit_code, _report = run_suite(
            manifest_path=manifest_path,
            output_path=output_path,
            repo_root=self.root,
            tests_root=self.tests_root,
            executor=FakeExecutor(),
            env=self.required_env,
            git_sha_resolver=lambda _root: "9" * 40,
        )
        self.assertNotEqual(exit_code, 0)
        self.assertTrue(output_path.is_file(), "validation failure must still write report")
        payload = json.loads(output_path.read_text(encoding="utf-8"))
        self.assertEqual(payload["gitShaFull"], "9" * 40)
        self.assertTrue(payload["failures"])


def run_self_tests() -> int:
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(DenialRunnerSelfTests)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--manifest",
        type=Path,
        default=DEFAULT_MANIFEST,
        help="Path to multi-org denial manifest JSON",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=None,
        help="Sanitized manifest-run.json output path",
    )
    parser.add_argument(
        "--repo-root",
        type=Path,
        default=REPO_ROOT,
        help="Repository root for git SHA resolution and cargo cwd",
    )
    parser.add_argument(
        "--tests-root",
        type=Path,
        default=SERVER_TESTS_ROOT,
        help="Integration tests directory containing {{binary}}.rs sources",
    )
    parser.add_argument("--self-test", action="store_true", help="Run hermetic contract tests")
    args = parser.parse_args(argv)

    if args.self_test:
        return run_self_tests()

    exit_code, _report = run_suite(
        manifest_path=args.manifest.resolve(),
        output_path=args.output.resolve() if args.output else None,
        repo_root=args.repo_root.resolve(),
        tests_root=args.tests_root.resolve(),
        executor=SubprocessExecutor(),
        env=os.environ.copy(),
    )
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
