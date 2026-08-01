#!/usr/bin/env python3
"""Phase 1C connected multi-org denial manifest runner.

Reads the denial manifest, validates executable rows against integration test
sources, groups rows by binary, runs each unique binary once via cargo test with
``--include-ignored``, scans bounded child output for foreign-marker leakage and
secret-shaped material, and writes a sanitized deterministic JSON report.

Leakage scanning limits
-----------------------
The runner scans child stdout/stderr for **fixture-declared indexed marker strings**
(``indexedMarkers``) and the ``objectKeyTemplate`` label. It deliberately does **not**
treat cross-org duplicate display names (shared collection/document titles) as leak
needles — those names are intentional oracles and may appear in ordinary passing
cargo output without indicating a tenancy breach.

Runtime UUIDs and other boot-time foreign identifiers are **not** inferred from
fixture or child output. Per-test ``assert_denial_no_leak`` in Rust remains the
authoritative runtime ID/name/key scan inside each integration test. This runner's
foreign-marker signal is: (1) nonzero child exit / timeout, plus (2) explicit
indexed marker strings or template needles appearing in captured output.

Hermetic ``--self-test`` uses temporary manifests/sources and a fake executor;
it never runs cargo, network, or real repo integration tests.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import os
import re
import signal
import subprocess
import sys
import tempfile
import unittest
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Mapping, Protocol, Sequence

REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_MANIFEST = REPO_ROOT / "crates/server/tests/fixtures/multi-org-denial.manifest.json"
SERVER_TESTS_REL = Path("crates/server/tests")
CARGO_PACKAGE = "fileconv-server"
REQUIRED_ENV_VAR = "MARKHAND_TEST_REQUIRED"
REQUIRED_ENV_VALUE = "1"
MAX_CAPTURE_BYTES = 256 * 1024
SNIPPET_HASH_BYTES = 4096
REPORT_SCHEMA_VERSION = 1
DEFAULT_BINARY_TIMEOUT_SECS = int(
    os.environ.get("PHASE1C_DENIAL_BINARY_TIMEOUT_SECS", "1800")
)

BINARY_ID_RE = re.compile(r"^[a-z][a-z0-9_]*$")
ALLOWED_STATUSES = frozenset({"executable", "na", "deferred"})
RUST_FN_DECL_RE = re.compile(
    r"(?:#\[[^\]]*\]\s*)*(?:pub\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)"
)

SENSITIVE_JSON_KEY_RE = re.compile(
    r"(?i)^(?:password|passwd|secret|token|authorization|api[_-]?key|access[_-]?key|"
    r"private[_-]?key|signing[_-]?key|client[_-]?secret|refresh[_-]?token|"
    r"database_url|jwt_secret|env|environment|"
    r"markhand_[a-z0-9_]*(?:secret|password|token|api_key|access_key|private_key|signing_key))$"
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
MARKHAND_ENV_ASSIGN_RE = re.compile(
    r"(?i)\b(MARKHAND_[A-Z0-9_]*(?:SECRET|PASSWORD|TOKEN|API_KEY|ACCESS_KEY|PRIVATE_KEY|SIGNING_KEY|DATABASE_URL)[A-Z0-9_]*)\s*[=:]\s*"
    r"([^\s'\"\\]+|'[^']*'|\"[^\"]*\")"
)


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
        return cls(version=int(raw["version"]), rows=tuple(ManifestRow.from_dict(row) for row in raw["rows"]))


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
    timed_out: bool = False


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
        default_factory=lambda: {"passed": True, "findings": []}
    )


class CommandExecutor(Protocol):
    def run(
        self,
        command: Sequence[str],
        *,
        cwd: Path,
        env: Mapping[str, str],
        timeout_secs: int,
    ) -> ChildResult: ...


class SubprocessExecutor:
    """Run cargo via argv list with bounded capture, timeout, and process-group kill."""

    def run(
        self,
        command: Sequence[str],
        *,
        cwd: Path,
        env: Mapping[str, str],
        timeout_secs: int,
    ) -> ChildResult:
        binary = _command_binary(command)
        process = subprocess.Popen(
            list(command),
            cwd=str(cwd),
            env=dict(env),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            start_new_session=True,
        )
        timed_out = False
        try:
            stdout, stderr = process.communicate(timeout=timeout_secs)
            exit_code = int(process.returncode or 0)
        except subprocess.TimeoutExpired:
            timed_out = True
            _kill_process_group(process.pid)
            stdout, stderr = process.communicate()
            exit_code = 124

        return ChildResult(
            binary=binary,
            exit_code=exit_code,
            stdout=bound_capture(stdout or ""),
            stderr=bound_capture(stderr or ""),
            timed_out=timed_out,
        )


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
        timeout_secs: int,
    ) -> ChildResult:
        del cwd, env, timeout_secs
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


def _kill_process_group(pid: int) -> None:
    try:
        os.killpg(pid, signal.SIGKILL)
    except ProcessLookupError:
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass


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


def resolve_tests_root(repo_root: Path) -> Path:
    return (repo_root / SERVER_TESTS_REL).resolve()


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


def _is_rust_ident_continue(ch: str) -> bool:
    return ch.isalnum() or ch == "_"


def _skip_nested_block_comment(source: str, index: int) -> int:
    """Return index after a nested ``/* ... */`` block (opening ``/*`` already consumed)."""
    depth = 1
    length = len(source)
    while index < length and depth > 0:
        if source[index] == "/" and index + 1 < length and source[index + 1] == "*":
            depth += 1
            index += 2
            continue
        if source[index] == "*" and index + 1 < length and source[index + 1] == "/":
            depth -= 1
            index += 2
            continue
        index += 1
    return index


def _skip_escaped_string_body(source: str, index: int, quote: str) -> int:
    length = len(source)
    while index < length:
        ch = source[index]
        if ch == "\\":
            index += 2
            continue
        if ch == quote:
            return index + 1
        index += 1
    return length


def _skip_raw_string(source: str, index: int, hash_count: int) -> int:
    """Return index after ``r#*\"...\"#*`` (opening quote already consumed)."""
    length = len(source)
    end_marker = f'"{"#" * hash_count}'
    while index < length:
        if source.startswith(end_marker, index):
            return index + len(end_marker)
        index += 1
    return length


def _skip_prefixed_string(source: str, index: int) -> tuple[int, bool]:
    """Skip ``b? r? #* \"...\" #*`` string forms; return (new_index, consumed)."""
    length = len(source)
    start = index
    byte_prefix = False
    if source[index] == "b":
        byte_prefix = True
        index += 1
        if index >= length:
            return index, False

    raw = False
    if source[index] == "r":
        raw = True
        index += 1
        if index >= length:
            return index, False

    hash_count = 0
    if raw:
        while index < length and source[index] == "#":
            hash_count += 1
            index += 1

    if index >= length or source[index] != '"':
        return start, False

    index += 1
    if raw:
        index = _skip_raw_string(source, index, hash_count)
    else:
        index = _skip_escaped_string_body(source, index, '"')
    return index, True


def _skip_char_or_lifetime(source: str, index: int) -> int:
    """Skip ``'a'`` char/byte-char literals or ``'static`` / ``'a`` lifetime annotations."""
    length = len(source)
    index += 1  # opening quote
    if index >= length:
        return index

    if source[index] == "\\":
        index += 1
        if index < length:
            index += 1
        if index < length and source[index] == "'":
            index += 1
        return index

    # Lifetime: 'ident without closing quote (e.g. 'static, 'a, '_)
    if source[index] == "_" or source[index].isalpha():
        index += 1
        while index < length and _is_rust_ident_continue(source[index]):
            index += 1
        return index

    # Char literal: consume until closing quote (handles escapes).
    return _skip_escaped_string_body(source, index, "'")


def strip_rust_comments_and_strings(source: str) -> str:
    """Remove comments and literals while preserving newlines and declaration structure."""
    out: list[str] = []
    index = 0
    length = len(source)

    while index < length:
        ch = source[index]
        nxt = source[index + 1] if index + 1 < length else ""

        if ch == "/" and nxt == "/":
            while index < length and source[index] != "\n":
                index += 1
            continue

        if ch == "/" and nxt == "*":
            index = _skip_nested_block_comment(source, index + 2)
            continue

        if ch == "b" and nxt in {"'", '"'}:
            if nxt == '"':
                new_index, consumed = _skip_prefixed_string(source, index)
                if consumed:
                    out.append(" ")
                    index = new_index
                    continue
            index = _skip_char_or_lifetime(source, index + 1)  # byte char b'x'
            out.append(" ")
            continue

        if ch == "c" and nxt == '"':
            new_index, consumed = _skip_prefixed_string(source, index)
            if consumed:
                out.append(" ")
                index = new_index
                continue

        if ch in {"r", '"'} or (ch == "b" and nxt == "r"):
            new_index, consumed = _skip_prefixed_string(source, index)
            if consumed:
                out.append(" ")
                index = new_index
                continue

        if ch == "'":
            index = _skip_char_or_lifetime(source, index)
            out.append(" ")
            continue

        out.append(ch)
        index += 1

    return "".join(out)


def extract_rust_test_names(source: str) -> set[str]:
    cleaned = strip_rust_comments_and_strings(source)
    return set(RUST_FN_DECL_RE.findall(cleaned))


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
            errors.append(f"manifest row {row.id} missing integration source at {source_path}")
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
    """Indexed marker strings and object-key template only — not shared display names."""
    needles: list[ForeignNeedle] = []
    for value in fixture.indexed_markers.values():
        if value:
            needles.append(ForeignNeedle("marker_string", value))
    if fixture.object_key_template:
        needles.append(ForeignNeedle("object_key", fixture.object_key_template))
    return needles


def scan_for_foreign_markers(
    text: str,
    needles: Sequence[ForeignNeedle],
) -> list[LeakFinding]:
    findings: list[LeakFinding] = []
    lowered = text.lower()
    for needle in needles:
        if not needle.value:
            continue
        variants = {needle.value, needle.value.lower(), needle.value.upper()}
        if any(variant in lowered for variant in variants if variant):
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
    if PEM_RE.search(text):
        labels.append("pem_private_key")
    if MARKHAND_ENV_ASSIGN_RE.search(text):
        labels.append("markhand_env_assignment")
    return labels


def redact_text(text: str, *, fixture: DenialFixture | None = None) -> str:
    out = PEM_RE.sub("<REDACTED_PEM>", text)
    out = BEARER_RE.sub("Bearer <REDACTED_BEARER>", out)
    out = JWT_RE.sub("<REDACTED_JWT>", out)
    out = ASSIGN_SECRET_RE.sub(r"\1=<REDACTED_SECRET>", out)
    out = MARKHAND_ENV_ASSIGN_RE.sub(r"\1=<REDACTED_ENV>", out)
    out = DB_URL_RE.sub("<REDACTED_DATABASE_URL>", out)
    out = CI_SECRET_RE.sub(r"\1=<REDACTED_CI_SECRET>", out)
    if fixture is not None:
        for needle in static_foreign_needles(fixture):
            if needle.value:
                pattern = re.compile(re.escape(needle.value), re.IGNORECASE)
                out = pattern.sub(f"<REDACTED_{needle.category.upper()}>", out)
    return out


def _redact_json_value(value: Any, *, fixture: DenialFixture | None) -> Any:
    if isinstance(value, dict):
        out: dict[str, Any] = {}
        for key, item in value.items():
            if isinstance(key, str) and SENSITIVE_JSON_KEY_RE.match(key):
                out[key] = "<REDACTED>"
            else:
                out[key] = _redact_json_value(item, fixture=fixture)
        return out
    if isinstance(value, list):
        return [_redact_json_value(item, fixture=fixture) for item in value]
    if isinstance(value, str):
        return redact_text(value, fixture=fixture)
    return value


def redact_report_dict(
    report: dict[str, Any],
    *,
    fixture: DenialFixture | None = None,
) -> dict[str, Any]:
    redacted = _redact_json_value(copy.deepcopy(report), fixture=fixture)
    serialized = json.dumps(redacted, sort_keys=True)
    residual = scan_for_secret_shapes(serialized)
    if fixture is not None:
        for needle in static_foreign_needles(fixture):
            if needle.value and needle.value.lower() in serialized.lower():
                residual.append(needle.label())
    if residual:
        redacted.setdefault("redactionScan", {})
        if isinstance(redacted["redactionScan"], dict):
            redacted["redactionScan"]["passed"] = False
            findings = set(redacted["redactionScan"].get("findings") or [])
            findings.update(sorted(set(residual)))
            redacted["redactionScan"]["findings"] = sorted(findings)
    return redacted


def bound_capture(text: str, *, limit: int = MAX_CAPTURE_BYTES) -> str:
    encoded = text.encode("utf-8", errors="replace")
    if len(encoded) <= limit:
        return text
    truncated = encoded[:limit].decode("utf-8", errors="ignore")
    return truncated + "\n<output truncated>"


def sanitized_output_digest(text: str, *, fixture: DenialFixture | None = None) -> tuple[str, str]:
    redacted = redact_text(bound_capture(text), fixture=fixture)
    full_hash = _sha256_bytes(redacted.encode("utf-8"))
    snippet_hash = _short_hash(redacted[:SNIPPET_HASH_BYTES])
    return full_hash, snippet_hash


def build_deterministic_report_dict(report: RunReport) -> dict[str, Any]:
    findings = sorted(
        [finding.as_dict() for finding in report.findings],
        key=lambda item: (item["category"], item["label"], item["hash"]),
    )
    redaction_findings = sorted(report.redaction_scan.get("findings") or [])
    return {
        "schemaVersion": report.schema_version,
        "gitShaFull": report.git_sha_full,
        "manifestSha256": report.manifest_sha256,
        "executableCount": report.executable_count,
        "naCount": report.na_count,
        "deferredCount": report.deferred_count,
        "binariesRun": sorted(report.binaries_run),
        "failures": sorted(report.failures),
        "leakageCount": report.leakage_count,
        "findings": findings,
        "redactionScan": {
            "passed": bool(report.redaction_scan.get("passed", True)),
            "findings": redaction_findings,
        },
    }


def write_report_atomic(path: Path, report_dict: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = json.dumps(report_dict, indent=2, sort_keys=True) + "\n"
    temp_name: str | None = None
    try:
        fd, temp_name = tempfile.mkstemp(
            prefix=".manifest-run-",
            suffix=".json",
            dir=path.parent,
        )
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            handle.write(payload)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temp_name, path)
        dir_fd = os.open(path.parent, os.O_DIRECTORY)
        try:
            os.fsync(dir_fd)
        finally:
            os.close(dir_fd)
        temp_name = None
    finally:
        if temp_name is not None:
            try:
                os.unlink(temp_name)
            except OSError:
                pass


def execute_grouped_binaries(
    binaries: Sequence[str],
    *,
    executor: CommandExecutor,
    cwd: Path,
    env: Mapping[str, str],
    timeout_secs: int = DEFAULT_BINARY_TIMEOUT_SECS,
) -> list[ChildResult]:
    results: list[ChildResult] = []
    for binary in binaries:
        command = build_cargo_command(binary)
        results.append(
            executor.run(command, cwd=cwd, env=env, timeout_secs=timeout_secs)
        )
    return results


def assemble_report(
    *,
    manifest: DenialManifest,
    fixture: DenialFixture,
    child_results: Sequence[ChildResult],
    git_sha_full: str,
    manifest_sha256: str,
) -> RunReport:
    executable_count, na_count, deferred_count = count_rows_by_status(manifest)
    report = RunReport(
        git_sha_full=git_sha_full,
        manifest_sha256=manifest_sha256,
        executable_count=executable_count,
        na_count=na_count,
        deferred_count=deferred_count,
    )

    static_needles = static_foreign_needles(fixture)
    findings_map: dict[tuple[str, str, str], LeakFinding] = {}

    for result in child_results:
        report.binaries_run.append(result.binary)
        combined = result.stdout + result.stderr

        for finding in scan_for_foreign_markers(combined, static_needles):
            findings_map[(finding.category, finding.label, finding.hash)] = finding

        if result.timed_out or result.exit_code != 0:
            output_hash, snippet_hash = sanitized_output_digest(combined, fixture=fixture)
            suffix = " (timed out)" if result.timed_out else ""
            report.failures.append(
                f"binary {result.binary} exited {result.exit_code}{suffix}; "
                f"outputSha256={output_hash}; snippetHash={snippet_hash}"
            )

    report.findings = sorted(
        findings_map.values(),
        key=lambda item: (item.category, item.label, item.hash),
    )
    report.leakage_count = len(report.findings)

    payload = build_deterministic_report_dict(report)
    redacted = redact_report_dict(payload, fixture=fixture)
    serialized = json.dumps(redacted, sort_keys=True)
    secret_labels = scan_for_secret_shapes(serialized)
    marker_residuals: list[str] = []
    for needle in static_needles:
        if needle.value and needle.value.lower() in serialized.lower():
            marker_residuals.append(needle.label())
    redaction_findings = sorted(set(secret_labels + marker_residuals))
    report.redaction_scan = {
        "passed": not redaction_findings,
        "findings": redaction_findings,
    }
    if redaction_findings:
        report.failures.append(
            "redaction scan detected residual secret or marker material: "
            + ", ".join(redaction_findings)
        )

    return report


def _safe_manifest_sha256(manifest_path: Path) -> str:
    try:
        return _sha256_file(manifest_path)
    except OSError:
        return ""


def _write_sanitized_report(
    report: RunReport,
    output_path: Path,
    *,
    fixture: DenialFixture | None,
) -> bool:
    try:
        payload = build_deterministic_report_dict(report)
        redacted = redact_report_dict(payload, fixture=fixture)
        write_report_atomic(output_path, redacted)
        return True
    except OSError:
        return False


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
    git_sha_full = git_sha_resolver(repo_root)
    manifest_sha256 = _safe_manifest_sha256(manifest_path)
    report = RunReport(git_sha_full=git_sha_full, manifest_sha256=manifest_sha256)
    fixture: DenialFixture | None = None
    write_ok = output_path is None

    def finalize(exit_code: int) -> tuple[int, RunReport]:
        nonlocal write_ok
        if output_path is not None:
            write_ok = _write_sanitized_report(report, output_path, fixture=fixture)
            if not write_ok:
                report.failures.append("failed to write sanitized report artifact")
        if exit_code == 0 and (report.failures or report.leakage_count > 0):
            exit_code = 1
        if exit_code == 0 and not write_ok:
            exit_code = 1
        return exit_code, report

    try:
        validate_required_env(runtime_env)
    except ValueError as exc:
        report.failures.append(str(exc))
        return finalize(2)

    try:
        manifest = load_manifest(manifest_path)
    except (OSError, json.JSONDecodeError, KeyError, TypeError, ValueError) as exc:
        report.failures.append(f"manifest load failed: {type(exc).__name__}")
        return finalize(2)

    report.executable_count, report.na_count, report.deferred_count = count_rows_by_status(manifest)

    validation_errors = validate_manifest_schema(manifest)
    validation_errors.extend(validate_executable_sources(manifest, tests_root=tests_root))
    if validation_errors:
        report.failures.extend(sorted(validation_errors))
        return finalize(2)

    fixture_path = fixture_path_for_manifest(manifest_path)
    try:
        fixture = load_fixture(fixture_path)
    except (OSError, json.JSONDecodeError, KeyError, TypeError, ValueError) as exc:
        report.failures.append(f"fixture load failed: {type(exc).__name__}")
        return finalize(2)

    grouped = group_executable_rows_by_binary(manifest)
    child_results = execute_grouped_binaries(
        list(grouped.keys()),
        executor=executor,
        cwd=repo_root,
        env=runtime_env,
    )
    report = assemble_report(
        manifest=manifest,
        fixture=fixture,
        child_results=child_results,
        git_sha_full=git_sha_full,
        manifest_sha256=manifest_sha256,
    )
    return finalize(0 if not report.failures and report.leakage_count == 0 else 1)


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

    def test_rust_test_name_parser_ignores_comments_and_strings(self) -> None:
        source = '''
// async fn decoy_in_comment() {}
/// Always fails for one specific job's outbox event with fn decoy_in_doc() {}
const IGNORED = "fn fake_in_string() {}";
/* block fn block_decoy() { 'published' } */
#[tokio::test]
async fn real_integration_test() {}
fn helper_only() {}
'''
        names = extract_rust_test_names(source)
        self.assertIn("real_integration_test", names)
        self.assertNotIn("decoy_in_comment", names)
        self.assertNotIn("fake_in_string", names)
        self.assertNotIn("block_decoy", names)
        self.assertNotIn("decoy_in_doc", names)

    def test_rust_lexer_handles_literal_forms_before_target_fn(self) -> None:
        snippets: list[tuple[str, str, list[str]]] = [
            (
                "normal_string",
                '''
const X = "VALUES ($1, 'job.enqueued', $2)";
#[tokio::test]
async fn target_normal_string() {}
''',
                ["target_normal_string"],
            ),
            (
                "static_lifetime",
                '''
fn helper(ext: &'static str) {}
#[tokio::test]
async fn target_static_lifetime() {}
''',
                ["helper", "target_static_lifetime"],
            ),
            (
                "byte_string",
                '''
put_bytes(b"TAMPERED MARKDOWN BYTES", "text/markdown; charset=utf-8");
#[tokio::test]
async fn target_byte_string() {}
''',
                ["target_byte_string"],
            ),
            (
                "raw_string",
                '''
const SQL: &str = r#"SELECT 'published' FROM "t""#;
#[tokio::test]
async fn target_raw_string() {}
''',
                ["target_raw_string"],
            ),
            (
                "raw_string_hashes",
                '''
const SQL: &str = r##"edge "quote" and 'published'"##;
#[tokio::test]
async fn target_raw_string_hashes() {}
''',
                ["target_raw_string_hashes"],
            ),
            (
                "byte_raw_string",
                '''
const SQL: &str = br#"byte 'published' raw"#;
#[tokio::test]
async fn target_byte_raw_string() {}
''',
                ["target_byte_raw_string"],
            ),
            (
                "char_literal",
                r"""
const C: char = '\'';
#[tokio::test]
async fn target_char_literal() {}
""",
                ["target_char_literal"],
            ),
            (
                "nested_block_comment",
                r"""
/* outer /* inner */ trailing */
#[tokio::test]
async fn target_nested_block_comment() {}
""",
                ["target_nested_block_comment"],
            ),
        ]
        for label, source, expected in snippets:
            names = extract_rust_test_names(source)
            for fn_name in expected:
                self.assertIn(fn_name, names, f"{label}: missing {fn_name}")
            self.assertNotIn("nested_decoy", names, label)

    def test_repo_jobs_source_contains_org_isolation_test(self) -> None:
        source = (REPO_ROOT / "crates/server/tests/jobs.rs").read_text(encoding="utf-8")
        names = extract_rust_test_names(source)
        self.assertIn("org_isolation_prevents_cross_org_claim_see_and_mutate", names)

    def test_repo_citation_authz_matrix_source_contains_live_citation_test(self) -> None:
        source = (REPO_ROOT / "crates/server/tests/citation_authz_matrix.rs").read_text(
            encoding="utf-8"
        )
        names = extract_rust_test_names(source)
        self.assertIn("live_citation_authz_expiry_replay_idor_and_immediate_deny", names)

    def test_repo_manifest_executable_rows_resolve_without_cargo(self) -> None:
        manifest = load_manifest(DEFAULT_MANIFEST)
        errors = validate_manifest_schema(manifest)
        errors.extend(
            validate_executable_sources(manifest, tests_root=resolve_tests_root(REPO_ROOT))
        )
        self.assertEqual(errors, [], "\n".join(errors))
        executable = sum(1 for row in manifest.rows if row.status == "executable")
        self.assertEqual(executable, 74)

    def test_shared_display_names_are_not_static_leak_needles(self) -> None:
        fixture = load_fixture(self.fixture_path)
        needles = static_foreign_needles(fixture)
        values = {needle.value for needle in needles}
        self.assertIn("phase1c-marker-beta", values)
        self.assertNotIn("Shared Contract Collection", values)
        self.assertNotIn("Shared Contract Document", values)

    def test_missing_required_env_fails_closed(self) -> None:
        manifest_path = self.write_manifest(
            [
                {
                    "id": "na-export",
                    "guardInventoryRef": "export_route_absent",
                    "layer": "http",
                    "status": "na",
                    "naCategory": "export_route_absent",
                }
            ]
        )
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
        payload = json.loads((self.root / "report.json").read_text(encoding="utf-8"))
        self.assertEqual(payload["gitShaFull"], "a" * 40)

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
            any("api_http_contracts" in failure and "101" in failure for failure in report.failures),
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
        serialized = (self.root / "report.json").read_text(encoding="utf-8")
        self.assertNotIn("phase1c-marker-beta", serialized)

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
        results = execute_grouped_binaries(
            list(grouped.keys()),
            executor=executor,
            cwd=self.root,
            env=self.required_env,
        )
        self.assertEqual([result.binary for result in results], ["alpha_binary", "zebra_binary"])
        self.assertEqual(len(executor.calls), 2)
        self.assertEqual(_command_binary(executor.calls[0]), "alpha_binary")

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
            "redactionScan": {"passed": True, "findings": []},
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
            findings=[LeakFinding("foreign_marker", "marker_string:abc", "a" * 64)],
            redaction_scan={"passed": True, "findings": []},
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
        help="Path to multi-org denial manifest JSON (relative to invocation cwd)",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=None,
        help="Sanitized manifest-run.json output path (relative to invocation cwd)",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="Run hermetic contract tests",
    )
    args = parser.parse_args(argv)

    if args.self_test:
        return run_self_tests()

    cwd = Path.cwd()
    manifest_path = args.manifest if args.manifest.is_absolute() else cwd / args.manifest
    manifest_path = manifest_path.resolve()

    output_path: Path | None = None
    if args.output is not None:
        output_path = args.output if args.output.is_absolute() else cwd / args.output
        output_path = output_path.resolve()

    repo_root = REPO_ROOT
    tests_root = resolve_tests_root(repo_root)

    exit_code, _report = run_suite(
        manifest_path=manifest_path,
        output_path=output_path,
        repo_root=repo_root,
        tests_root=tests_root,
        executor=SubprocessExecutor(),
        env=os.environ.copy(),
    )
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
