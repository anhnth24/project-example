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
import io
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
from unittest.mock import patch

REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_MANIFEST = REPO_ROOT / "crates/server/tests/fixtures/multi-org-denial.manifest.json"
SERVER_TESTS_REL = Path("crates/server/tests")
CARGO_PACKAGE = "fileconv-server"
REQUIRED_ENV_VAR = "MARKHAND_TEST_REQUIRED"
REQUIRED_ENV_VALUE = "1"
MAX_CAPTURE_BYTES = 256 * 1024
SNIPPET_HASH_BYTES = 4096
FAILURE_ECHO_TAIL_CHARS = 20_000
REPORT_SCHEMA_VERSION = 1
DEFAULT_BINARY_TIMEOUT_SECS = int(
    os.environ.get("PHASE1C_DENIAL_BINARY_TIMEOUT_SECS", "1800")
)

BINARY_ID_RE = re.compile(r"^[a-z][a-z0-9_]*$")
ALLOWED_STATUSES = frozenset({"executable", "na", "deferred"})
MANIFEST_ROOT_KEYS = frozenset({"version", "rows"})
MANIFEST_ROW_KEYS = frozenset(
    {
        "id",
        "binary",
        "testName",
        "operationId",
        "guardInventoryRef",
        "layer",
        "status",
        "coverageState",
        "evidenceRole",
        "naCategory",
        "coverageNote",
        "deferredTask",
    }
)
RUST_FN_DECL_RE = re.compile(
    r"(?:#\[[^\]]*\]\s*)*(?:pub\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)"
)

SENSITIVE_JSON_KEY_RE = re.compile(
    r"(?i)^(?:password|passwd|secret|token|authorization|api[_-]?key|access[_-]?key|"
    r"access[_-]?token|accesstoken|"
    r"private[_-]?key|signing[_-]?key|client[_-]?secret|refresh[_-]?token|"
    r"database_url|jwt_secret|env|environment|"
    r"markhand_[a-z0-9_]*(?:secret|password|token|api_key|access_key|private_key|signing_key))$"
)

STRUCTURED_REDACTED = "__REDACTED__"

SENSITIVE_JSON_KV_REDACT_RE = re.compile(
    r'(?i)"('
    r"access[_-]?token|refresh[_-]?token|"
    r"password|passwd|secret|token|api[_-]?key|"
    r"access[_-]?key|private[_-]?key|client[_-]?secret"
    r')"\s*:\s*"[^"\\]*(?:\\.[^"\\]*)*"'
)

SENSITIVE_JSON_KV_RE = re.compile(
    r'(?i)"('
    r"access[_-]?token|refresh[_-]?token|"
    r"password|passwd|secret|token|api[_-]?key|"
    r"access[_-]?key|private[_-]?key|client[_-]?secret"
    r')"\s*:\s*"(?!\[REDACTED\]|__REDACTED__|<REDACTED>)[^"]*"'
)

COOKIE_HEADER_REDACT_RE = re.compile(r"(?i)(?:Set-)?Cookie:\s*[^\n\r]+")

BEARER_RE = re.compile(r"(?i)\bBearer\s+[A-Za-z0-9._\-+=/]{8,}")
AUTH_BASIC_REDACT_RE = re.compile(r"(?i)Authorization:\s*Basic\s+\S+")
AUTH_BASIC_RE = re.compile(r"(?i)Authorization:\s*Basic\s+(?!\[REDACTED\])\S+")
COOKIE_HEADER_RE = re.compile(
    r"(?i)(?:Set-)?Cookie:\s+[^;\n\r]*=\s*(?!\[REDACTED\])\S"
)
JWT_RE = re.compile(
    r"\beyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\b"
)
ASSIGN_SECRET_RE = re.compile(
    r"(?i)\b("
    r"refresh[_-]?token|access[_-]?(?:key|token)|"
    r"password|passwd|secret|api[_-]?key|private[_-]?key|token"
    r")\s*[=:]\s*"
    r"(?!\[REDACTED\])(?!\[REDACTED-jwt\])(?!\[REDACTED-db-url\])"
    r"([^\s'\"\\]+|'[^']*'|\"[^\"]*\")"
)
DB_URL_RE = re.compile(r"(?i)\b(postgres(?:ql)?|mysql|mongodb|redis)://[^\s'\"]+")
CI_SECRET_RE = re.compile(
    r"(?i)\b(GITHUB_TOKEN|AWS_SECRET_ACCESS_KEY|NPM_TOKEN|PYPI_API_TOKEN)\s*[=:]\s*"
    r"(?!\[REDACTED\])\S+"
)
PEM_RE = re.compile(
    r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?-----END [A-Z0-9 ]*PRIVATE KEY-----",
    re.DOTALL,
)
MARKHAND_ENV_ASSIGN_RE = re.compile(
    r"(?i)\b(MARKHAND_[A-Z0-9_]*(?:SECRET|PASSWORD|TOKEN|API_KEY|ACCESS_KEY|PRIVATE_KEY|SIGNING_KEY|DATABASE_URL)[A-Z0-9_]*)\s*[=:]\s*"
    r"(?!\[REDACTED\])(?!\[REDACTED-jwt\])(?!\[REDACTED-db-url\])"
    r"([^\s'\"\\]+|'[^']*'|\"[^\"]*\")"
)


@dataclass(frozen=True)
class ManifestRow:
    id: object
    status: object
    guard_inventory_ref: object
    layer: object
    binary: object | None = None
    test_name: object | None = None
    operation_id: object | None = None
    na_category: object | None = None
    coverage_state: object | None = None
    evidence_role: object | None = None
    coverage_note: object | None = None
    deferred_task: object | None = None

    @classmethod
    def from_dict(cls, raw: Mapping[str, Any]) -> ManifestRow:
        return cls(
            id=raw.get("id"),
            status=raw.get("status"),
            guard_inventory_ref=raw.get("guardInventoryRef"),
            layer=raw.get("layer"),
            binary=raw.get("binary"),
            test_name=raw.get("testName"),
            operation_id=raw.get("operationId"),
            na_category=raw.get("naCategory"),
            coverage_state=raw.get("coverageState"),
            evidence_role=raw.get("evidenceRole"),
            coverage_note=raw.get("coverageNote"),
            deferred_task=raw.get("deferredTask"),
        )


@dataclass(frozen=True)
class DenialManifest:
    version: int
    rows: tuple[ManifestRow, ...]

    @classmethod
    def from_dict(cls, raw: Mapping[str, Any]) -> DenialManifest:
        manifest, errors = parse_manifest_document(raw)
        if errors:
            raise ValueError("; ".join(errors))
        assert manifest is not None
        return manifest


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

    def __init__(
        self,
        outcomes: Mapping[str, FakeExecutorOutcome] | None = None,
        *,
        list_outcomes: Mapping[str, FakeExecutorOutcome] | None = None,
    ) -> None:
        self.outcomes = dict(outcomes or {})
        self.list_outcomes = dict(list_outcomes or {})
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
        source = self.list_outcomes if "--list" in command else self.outcomes
        outcome = source.get(binary, FakeExecutorOutcome())
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


def _is_strict_int(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def parse_manifest_document(raw: object) -> tuple[DenialManifest | None, list[str]]:
    errors: list[str] = []
    if not isinstance(raw, dict):
        return None, ["manifest root must be an object"]

    root_keys = set(raw.keys())
    missing_root = MANIFEST_ROOT_KEYS - root_keys
    if missing_root:
        errors.append("manifest root missing keys: " + ", ".join(sorted(missing_root)))
    unexpected_root = root_keys - MANIFEST_ROOT_KEYS
    if unexpected_root:
        errors.append("manifest root has unexpected keys: " + ", ".join(sorted(unexpected_root)))

    version: int | None = None
    if "version" in raw:
        version_value = raw["version"]
        if not _is_strict_int(version_value):
            errors.append("manifest version must be an integer")
        else:
            version = version_value

    parsed_rows: list[ManifestRow] = []
    if "rows" in raw:
        rows_raw = raw["rows"]
        if not isinstance(rows_raw, list):
            errors.append("manifest rows must be a list")
        else:
            for index, row_raw in enumerate(rows_raw):
                row_label = f"rows[{index}]"
                if not isinstance(row_raw, dict):
                    errors.append(f"manifest {row_label} must be an object")
                    continue
                unexpected_row = set(row_raw.keys()) - MANIFEST_ROW_KEYS
                if unexpected_row:
                    errors.append(
                        f"manifest {row_label} has unexpected keys: "
                        + ", ".join(sorted(unexpected_row))
                    )
                parsed_rows.append(ManifestRow.from_dict(row_raw))

    if errors or version is None:
        return None, errors
    return DenialManifest(version=version, rows=tuple(parsed_rows)), []


def load_manifest(path: Path) -> DenialManifest:
    raw = json.loads(path.read_text(encoding="utf-8"))
    manifest, errors = parse_manifest_document(raw)
    if errors:
        raise ValueError("; ".join(errors))
    assert manifest is not None
    return manifest


def load_fixture(path: Path) -> DenialFixture:
    raw = json.loads(path.read_text(encoding="utf-8"))
    return DenialFixture.from_dict(raw)


def fixture_path_for_manifest(manifest_path: Path) -> Path:
    return manifest_path.with_name("multi-org-denial.fixture.json")


def _manifest_row_label(row: ManifestRow, index: int) -> str:
    if isinstance(row.id, str) and row.id.strip():
        return row.id
    return f"rows[{index}]"


def _validate_required_string_field(
    value: object,
    field_name: str,
    row_label: str,
) -> tuple[str | None, list[str]]:
    if not isinstance(value, str):
        return None, [f"manifest row {row_label} {field_name} must be a string"]
    if not value.strip():
        return None, [f"manifest row {row_label} missing {field_name}"]
    return value, []


def _validate_optional_string_field(
    value: object,
    field_name: str,
    row_label: str,
) -> list[str]:
    if value is None:
        return []
    if not isinstance(value, str):
        return [f"manifest row {row_label} {field_name} must be a string or omitted"]
    if not value.strip():
        return [f"manifest row {row_label} {field_name} must not be empty when present"]
    return []


def validate_manifest_schema(manifest: DenialManifest) -> list[str]:
    errors: list[str] = []
    if manifest.version < 1:
        errors.append(f"manifest version must be >= 1, got {manifest.version}")
    if not manifest.rows:
        errors.append("manifest rows must not be empty")

    seen_ids: set[str] = set()
    executable_rows = 0
    for index, row in enumerate(manifest.rows):
        row_label = _manifest_row_label(row, index)
        row_errors: list[str] = []

        id_value, id_errors = _validate_required_string_field(row.id, "id", row_label)
        row_errors.extend(id_errors)
        if id_value is not None:
            row_label = id_value

        status_value, status_errors = _validate_required_string_field(row.status, "status", row_label)
        row_errors.extend(status_errors)

        _, layer_errors = _validate_required_string_field(row.layer, "layer", row_label)
        row_errors.extend(layer_errors)

        _, guard_errors = _validate_required_string_field(
            row.guard_inventory_ref,
            "guardInventoryRef",
            row_label,
        )
        row_errors.extend(guard_errors)

        row_errors.extend(_validate_optional_string_field(row.binary, "binary", row_label))
        row_errors.extend(_validate_optional_string_field(row.test_name, "testName", row_label))
        row_errors.extend(_validate_optional_string_field(row.operation_id, "operationId", row_label))
        row_errors.extend(_validate_optional_string_field(row.na_category, "naCategory", row_label))
        row_errors.extend(_validate_optional_string_field(row.coverage_state, "coverageState", row_label))
        row_errors.extend(_validate_optional_string_field(row.evidence_role, "evidenceRole", row_label))
        row_errors.extend(_validate_optional_string_field(row.coverage_note, "coverageNote", row_label))
        row_errors.extend(_validate_optional_string_field(row.deferred_task, "deferredTask", row_label))

        if row_errors:
            errors.extend(row_errors)
            continue

        assert id_value is not None
        assert status_value is not None

        if status_value not in ALLOWED_STATUSES:
            errors.append(f"manifest row {row_label} has unknown status {status_value!r}")
            continue
        if status_value == "deferred":
            errors.append(f"manifest row {row_label} has unresolved deferred status")
        if id_value in seen_ids:
            errors.append(f"duplicate manifest row id {id_value}")
        seen_ids.add(id_value)

        if status_value == "na":
            if not isinstance(row.na_category, str) or not row.na_category.strip():
                errors.append(f"manifest row {row_label} missing naCategory")
            if row.coverage_state == "deferred":
                errors.append(
                    f"manifest row {row_label} cannot combine status=na with coverageState=deferred"
                )
        elif status_value == "executable":
            executable_rows += 1
            if row.coverage_state == "deferred":
                errors.append(
                    f"executable manifest row {row_label} cannot declare coverageState=deferred"
                )
            if not isinstance(row.operation_id, str) or not row.operation_id.strip():
                errors.append(f"executable manifest row {row_label} missing operationId")
            if not isinstance(row.binary, str) or not row.binary.strip():
                errors.append(f"executable manifest row {row_label} missing binary")
            elif not is_safe_binary_identifier(row.binary):
                errors.append(
                    f"executable manifest row {row_label} binary {row.binary!r} is not a safe "
                    "Rust integration target identifier"
                )
            if not isinstance(row.test_name, str) or not row.test_name.strip():
                errors.append(f"executable manifest row {row_label} missing testName")

    if manifest.rows and executable_rows == 0:
        errors.append("manifest must declare at least one executable row for denial gate coverage")
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
    executable = sum(
        1 for row in manifest.rows if isinstance(row.status, str) and row.status == "executable"
    )
    na = sum(1 for row in manifest.rows if isinstance(row.status, str) and row.status == "na")
    deferred = sum(
        1 for row in manifest.rows if isinstance(row.status, str) and row.status == "deferred"
    )
    return executable, na, deferred


def group_executable_rows_by_binary(manifest: DenialManifest) -> dict[str, list[ManifestRow]]:
    grouped: dict[str, list[ManifestRow]] = {}
    for row in manifest.rows:
        if row.status != "executable":
            continue
        assert row.binary is not None
        grouped.setdefault(row.binary, []).append(row)
    return dict(sorted(grouped.items()))


def build_cargo_list_command(binary: str) -> list[str]:
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
        "--list",
        "--include-ignored",
    ]


def parse_cargo_test_list_output(text: str) -> set[str]:
    """Return full libtest harness names registered in the Cargo test harness."""
    registered: set[str] = set()
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped:
            continue
        match = re.match(r"^(.+): (?:test|ignored)$", stripped)
        if match is None:
            continue
        registered.add(match.group(1))
    return registered


def validate_executable_harness_registration(
    manifest: DenialManifest,
    *,
    executor: CommandExecutor,
    repo_root: Path,
    env: Mapping[str, str],
    timeout_secs: int = DEFAULT_BINARY_TIMEOUT_SECS,
) -> list[str]:
    errors: list[str] = []
    cache: dict[str, set[str] | None] = {}

    for row in manifest.rows:
        if row.status != "executable":
            continue
        assert row.binary is not None
        if row.binary in cache:
            continue
        if not is_safe_binary_identifier(row.binary):
            cache[row.binary] = None
            continue

        command = build_cargo_list_command(row.binary)
        result = executor.run(
            command,
            cwd=repo_root,
            env=env,
            timeout_secs=timeout_secs,
        )
        if result.timed_out or result.exit_code != 0:
            errors.append(
                f"integration binary {row.binary} cargo test --list failed "
                f"(exit {result.exit_code}{' timed out' if result.timed_out else ''})"
            )
            cache[row.binary] = None
            continue
        cache[row.binary] = parse_cargo_test_list_output(result.stdout)

    for row in manifest.rows:
        if row.status != "executable":
            continue
        assert row.binary is not None
        assert row.test_name is not None
        registered = cache.get(row.binary)
        if registered is None:
            continue
        if row.test_name not in registered:
            errors.append(
                f"manifest row {row.id} executable test not registered in cargo harness "
                f"for binary {row.binary}"
            )
    return errors


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
    if AUTH_BASIC_RE.search(text):
        labels.append("basic_auth")
    if COOKIE_HEADER_RE.search(text):
        labels.append("cookie_header")
    if SENSITIVE_JSON_KV_RE.search(text):
        labels.append("json_credential")
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
    out = PEM_RE.sub("<redacted-pem>", text)
    out = BEARER_RE.sub("Bearer [REDACTED]", out)
    out = AUTH_BASIC_REDACT_RE.sub("Authorization: Basic [REDACTED]", out)
    out = COOKIE_HEADER_REDACT_RE.sub("Cookie: [REDACTED]", out)
    out = JWT_RE.sub("[REDACTED-jwt]", out)
    out = SENSITIVE_JSON_KV_REDACT_RE.sub(r'"\1":"[REDACTED]"', out)
    out = ASSIGN_SECRET_RE.sub(r"\1=[REDACTED]", out)
    out = MARKHAND_ENV_ASSIGN_RE.sub(r"\1=[REDACTED]", out)
    out = DB_URL_RE.sub("[REDACTED-db-url]", out)
    out = CI_SECRET_RE.sub(r"\1=[REDACTED]", out)
    if fixture is not None:
        for needle in static_foreign_needles(fixture):
            if needle.value:
                pattern = re.compile(re.escape(needle.value), re.IGNORECASE)
                out = pattern.sub(f"[REDACTED-{needle.category}]", out)
    return out


def _redact_json_value(value: Any, *, fixture: DenialFixture | None) -> Any:
    if isinstance(value, dict):
        out: dict[str, Any] = {}
        for key, item in value.items():
            if isinstance(key, str) and SENSITIVE_JSON_KEY_RE.match(key):
                out[key] = STRUCTURED_REDACTED
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


def emit_redacted_failure_output(
    child_results: Sequence[ChildResult],
    *,
    fixture: DenialFixture | None,
    stream: Any = None,
    tail_chars: int = FAILURE_ECHO_TAIL_CHARS,
) -> None:
    """Echo a redacted, bounded output tail for each failed child to stderr.

    The sanitized artifact intentionally keeps hashes only, which made CI
    failures undiagnosable from the workflow log (run 30778769464: `binary
    multi_org_denial exited 101` with no way to see which test panicked).
    This echo goes through the same redaction pipeline as the artifact, is
    suppressed entirely if residual secret shapes survive redaction, and
    never touches the report schema.
    """
    if stream is None:
        stream = sys.stderr
    for result in child_results:
        if not result.timed_out and result.exit_code == 0:
            continue
        combined = result.stdout + result.stderr
        redacted = redact_text(bound_capture(combined), fixture=fixture)
        tail = redacted[-tail_chars:]
        if scan_for_secret_shapes(tail):
            tail = "<suppressed: residual secret shapes survived redaction>"
        suffix = " (timed out)" if result.timed_out else ""
        stream.write(
            f"--- redacted output tail: binary {result.binary} "
            f"exited {result.exit_code}{suffix} ---\n"
            f"{tail}\n"
            f"--- end redacted output tail: {result.binary} ---\n"
        )


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
        raw_manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        report.failures.append(f"manifest load failed: {type(exc).__name__}")
        return finalize(2)

    manifest, parse_errors = parse_manifest_document(raw_manifest)
    if parse_errors:
        report.failures.extend(sorted(parse_errors))
        return finalize(2)
    assert manifest is not None

    report.executable_count, report.na_count, report.deferred_count = count_rows_by_status(manifest)

    validation_errors = validate_manifest_schema(manifest)
    if not validation_errors:
        validation_errors.extend(validate_executable_sources(manifest, tests_root=tests_root))
        validation_errors.extend(
            validate_executable_harness_registration(
                manifest,
                executor=executor,
                repo_root=repo_root,
                env=runtime_env,
            )
        )
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
    emit_redacted_failure_output(child_results, fixture=fixture)
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

    def test_declared_source_function_missing_harness_registration_rejected(self) -> None:
        self.write_source(
            "api_http_contracts",
            ["live_http_retrieval_refuses_foreign_collection_scope", "orphan_declared_not_registered"],
        )
        manifest_path = self.write_manifest(
            [
                {
                    "id": "denial-orphan",
                    "binary": "api_http_contracts",
                    "testName": "orphan_declared_not_registered",
                    "operationId": "ask",
                    "guardInventoryRef": "ask",
                    "layer": "http",
                    "status": "executable",
                }
            ]
        )
        manifest = load_manifest(manifest_path)
        executor = FakeExecutor(
            list_outcomes={
                "api_http_contracts": FakeExecutorOutcome(
                    stdout="live_http_retrieval_refuses_foreign_collection_scope: test\n",
                )
            }
        )
        errors = validate_executable_harness_registration(
            manifest,
            executor=executor,
            repo_root=self.root,
            env=self.required_env,
        )
        self.assertTrue(
            any(
                "denial-orphan" in error and "not registered in cargo harness" in error
                for error in errors
            ),
            errors,
        )
        self.assertTrue(all("orphan_declared_not_registered" not in error for error in errors), errors)

    def test_cargo_list_failure_rejected_before_execution(self) -> None:
        self.write_source("api_http_contracts", ["live_http_retrieval_refuses_foreign_collection_scope"])
        manifest_path = self.write_manifest(
            [
                {
                    "id": "denial-list-fail",
                    "binary": "api_http_contracts",
                    "testName": "live_http_retrieval_refuses_foreign_collection_scope",
                    "operationId": "ask",
                    "guardInventoryRef": "ask",
                    "layer": "http",
                    "status": "executable",
                }
            ]
        )
        manifest = load_manifest(manifest_path)
        executor = FakeExecutor(
            list_outcomes={
                "api_http_contracts": FakeExecutorOutcome(exit_code=101, stderr="compile error\n"),
            }
        )
        errors = validate_executable_harness_registration(
            manifest,
            executor=executor,
            repo_root=self.root,
            env=self.required_env,
        )
        self.assertTrue(
            any("cargo test --list failed" in error for error in errors),
            errors,
        )

    def test_ignored_harness_registration_counts_as_executable(self) -> None:
        self.write_source("api_http_contracts", ["ignored_live_case"])
        manifest_path = self.write_manifest(
            [
                {
                    "id": "denial-ignored",
                    "binary": "api_http_contracts",
                    "testName": "ignored_live_case",
                    "operationId": "ask",
                    "guardInventoryRef": "ask",
                    "layer": "http",
                    "status": "executable",
                }
            ]
        )
        manifest = load_manifest(manifest_path)
        executor = FakeExecutor(
            list_outcomes={
                "api_http_contracts": FakeExecutorOutcome(
                    stdout="ignored_live_case: ignored\n",
                )
            }
        )
        errors = validate_executable_harness_registration(
            manifest,
            executor=executor,
            repo_root=self.root,
            env=self.required_env,
        )
        self.assertEqual(errors, [], "\n".join(errors))

    def test_harness_validation_runs_before_grouped_execution(self) -> None:
        self.write_source("api_http_contracts", ["missing_from_harness"])
        manifest_path = self.write_manifest(
            [
                {
                    "id": "denial-preexec",
                    "binary": "api_http_contracts",
                    "testName": "missing_from_harness",
                    "operationId": "ask",
                    "guardInventoryRef": "ask",
                    "layer": "http",
                    "status": "executable",
                }
            ]
        )
        executor = FakeExecutor(
            list_outcomes={
                "api_http_contracts": FakeExecutorOutcome(stdout="other_test: test\n"),
            },
        )
        exit_code, report = run_suite(
            manifest_path=manifest_path,
            output_path=self.root / "report.json",
            repo_root=self.root,
            tests_root=self.tests_root,
            executor=executor,
            env=self.required_env,
            git_sha_resolver=lambda _root: "h" * 40,
        )
        self.assertEqual(exit_code, 2)
        assert report is not None
        self.assertTrue(
            any("not registered in cargo harness" in failure for failure in report.failures),
            report.failures,
        )
        self.assertEqual(len(executor.calls), 1)
        self.assertIn("--list", executor.calls[0])
        self.assertNotIn("--nocapture", executor.calls[0])

    def test_parse_cargo_test_list_output_keeps_full_libtest_harness_names(self) -> None:
        # Representative `cargo test --test api_http_contracts -- --list --include-ignored`
        # output (captured manually; self-tests must stay hermetic — no subprocess).
        sample = """
common::fts_visibility_diagnostic::tests::formatted_snapshot_includes_required_field_names: test
common::multi_org_denial::unit_tests::allow_success_rejects_401_but_accepts_2xx: test
live_http_retrieval_refuses_foreign_collection_scope: test
ignored_live_case: ignored

29 tests, 0 benchmarks
""".strip()
        registered = parse_cargo_test_list_output(sample)
        self.assertEqual(
            registered,
            {
                "common::fts_visibility_diagnostic::tests::formatted_snapshot_includes_required_field_names",
                "common::multi_org_denial::unit_tests::allow_success_rejects_401_but_accepts_2xx",
                "live_http_retrieval_refuses_foreign_collection_scope",
                "ignored_live_case",
            },
        )

    def test_nested_harness_name_does_not_certify_top_level_manifest_name(self) -> None:
        self.write_source("api_http_contracts", ["orphan_test"])
        manifest_path = self.write_manifest(
            [
                {
                    "id": "denial-collision",
                    "binary": "api_http_contracts",
                    "testName": "orphan_test",
                    "operationId": "ask",
                    "guardInventoryRef": "ask",
                    "layer": "http",
                    "status": "executable",
                }
            ]
        )
        manifest = load_manifest(manifest_path)
        executor = FakeExecutor(
            list_outcomes={
                "api_http_contracts": FakeExecutorOutcome(
                    stdout="nested::orphan_test: test\n",
                )
            }
        )
        errors = validate_executable_harness_registration(
            manifest,
            executor=executor,
            repo_root=self.root,
            env=self.required_env,
        )
        self.assertTrue(
            any("denial-collision" in error and "not registered in cargo harness" in error for error in errors),
            errors,
        )

    def test_exact_top_level_harness_name_certifies_manifest_row(self) -> None:
        self.write_source("api_http_contracts", ["orphan_test"])
        manifest_path = self.write_manifest(
            [
                {
                    "id": "denial-exact",
                    "binary": "api_http_contracts",
                    "testName": "orphan_test",
                    "operationId": "ask",
                    "guardInventoryRef": "ask",
                    "layer": "http",
                    "status": "executable",
                }
            ]
        )
        manifest = load_manifest(manifest_path)
        executor = FakeExecutor(
            list_outcomes={
                "api_http_contracts": FakeExecutorOutcome(
                    stdout="orphan_test: test\nnested::orphan_test: test\n",
                )
            }
        )
        errors = validate_executable_harness_registration(
            manifest,
            executor=executor,
            repo_root=self.root,
            env=self.required_env,
        )
        self.assertEqual(errors, [], "\n".join(errors))

    def _assert_schema_failure_without_cargo(
        self,
        rows: list[dict[str, Any]],
        *,
        expected_substring: str,
        git_sha: str,
    ) -> None:
        manifest_path = self.root / "manifest.json"
        manifest_path.write_text(json.dumps({"version": 1, "rows": rows}), encoding="utf-8")
        executor = FakeExecutor()
        exit_code, report = run_suite(
            manifest_path=manifest_path,
            output_path=self.root / "report.json",
            repo_root=self.root,
            tests_root=self.tests_root,
            executor=executor,
            env=self.required_env,
            git_sha_resolver=lambda _root: git_sha,
        )
        self.assertEqual(exit_code, 2)
        assert report is not None
        self.assertTrue(
            any(expected_substring in failure for failure in report.failures),
            report.failures,
        )
        self.assertEqual(executor.calls, [])
        self.assertTrue((self.root / "report.json").is_file())

    def _assert_manifest_document_failure_without_cargo(
        self,
        document: dict[str, Any],
        *,
        expected_substring: str,
        git_sha: str,
    ) -> None:
        manifest_path = self.root / "manifest.json"
        manifest_path.write_text(json.dumps(document), encoding="utf-8")
        executor = FakeExecutor()
        exit_code, report = run_suite(
            manifest_path=manifest_path,
            output_path=self.root / "report.json",
            repo_root=self.root,
            tests_root=self.tests_root,
            executor=executor,
            env=self.required_env,
            git_sha_resolver=lambda _root: git_sha,
        )
        self.assertEqual(exit_code, 2)
        assert report is not None
        self.assertTrue(
            any(expected_substring in failure for failure in report.failures),
            report.failures,
        )
        self.assertEqual(executor.calls, [])
        self.assertTrue((self.root / "report.json").is_file())

    def _executable_manifest_row(self, **overrides: object) -> dict[str, object]:
        row: dict[str, object] = {
            "id": "denial-sample",
            "binary": "api_http_contracts",
            "testName": "live_http_retrieval_refuses_foreign_collection_scope",
            "operationId": "ask",
            "guardInventoryRef": "ask",
            "layer": "http",
            "status": "executable",
        }
        row.update(overrides)
        return row

    def _na_manifest_row(self, **overrides: object) -> dict[str, object]:
        row: dict[str, object] = {
            "id": "na-sample",
            "guardInventoryRef": "export_route_absent",
            "layer": "http",
            "status": "na",
            "naCategory": "export_route_absent",
        }
        row.update(overrides)
        return row

    def test_malformed_non_string_id_fails_closed_without_cargo(self) -> None:
        self._assert_schema_failure_without_cargo(
            [self._executable_manifest_row(id=123)],
            expected_substring="id must be a string",
            git_sha="m1" + "0" * 38,
        )

    def test_malformed_non_string_status_fails_closed_without_cargo(self) -> None:
        self._assert_schema_failure_without_cargo(
            [self._executable_manifest_row(status=["executable"])],
            expected_substring="status must be a string",
            git_sha="m2" + "0" * 38,
        )

    def test_malformed_non_string_guard_inventory_ref_fails_closed_without_cargo(self) -> None:
        self._assert_schema_failure_without_cargo(
            [self._executable_manifest_row(guardInventoryRef={"bad": True})],
            expected_substring="guardInventoryRef must be a string",
            git_sha="m3" + "0" * 38,
        )

    def test_malformed_non_string_layer_fails_closed_without_cargo(self) -> None:
        self._assert_schema_failure_without_cargo(
            [self._executable_manifest_row(layer=404)],
            expected_substring="layer must be a string",
            git_sha="m4" + "0" * 38,
        )

    def test_malformed_non_string_na_category_fails_closed_without_cargo(self) -> None:
        self._assert_schema_failure_without_cargo(
            [self._na_manifest_row(naCategory=["export_route_absent"])],
            expected_substring="naCategory must be a string or omitted",
            git_sha="m5" + "0" * 38,
        )

    def test_malformed_non_string_operation_id_fails_closed_without_cargo(self) -> None:
        self._assert_schema_failure_without_cargo(
            [self._executable_manifest_row(operationId=999)],
            expected_substring="operationId must be a string or omitted",
            git_sha="m6" + "0" * 38,
        )

    def test_malformed_non_string_coverage_state_fails_closed_without_cargo(self) -> None:
        self._assert_schema_failure_without_cargo(
            [self._executable_manifest_row(coverageState={"state": "complete"})],
            expected_substring="coverageState must be a string or omitted",
            git_sha="m7" + "0" * 38,
        )

    def test_all_na_manifest_fails_closed_without_cargo(self) -> None:
        self._assert_schema_failure_without_cargo(
            [
                self._na_manifest_row(id="na-export-route-absent"),
                self._na_manifest_row(
                    id="na-autocomplete-route-absent",
                    guardInventoryRef="autocomplete_route_absent",
                    naCategory="autocomplete_route_absent",
                ),
            ],
            expected_substring="at least one executable row",
            git_sha="m8" + "0" * 38,
        )

    def test_string_manifest_version_fails_closed_without_cargo(self) -> None:
        self._assert_manifest_document_failure_without_cargo(
            {"version": "1", "rows": [self._executable_manifest_row()]},
            expected_substring="manifest version must be an integer",
            git_sha="n1" + "0" * 38,
        )

    def test_bool_manifest_version_fails_closed_without_cargo(self) -> None:
        self._assert_manifest_document_failure_without_cargo(
            {"version": True, "rows": [self._executable_manifest_row()]},
            expected_substring="manifest version must be an integer",
            git_sha="n2" + "0" * 38,
        )

    def test_non_list_manifest_rows_fails_closed_without_cargo(self) -> None:
        self._assert_manifest_document_failure_without_cargo(
            {"version": 1, "rows": {"bad": True}},
            expected_substring="manifest rows must be a list",
            git_sha="n3" + "0" * 38,
        )

    def test_non_object_manifest_row_fails_closed_without_cargo(self) -> None:
        self._assert_manifest_document_failure_without_cargo(
            {"version": 1, "rows": ["not-an-object"]},
            expected_substring="manifest rows[0] must be an object",
            git_sha="n4" + "0" * 38,
        )

    def test_unexpected_manifest_root_key_fails_closed_without_cargo(self) -> None:
        self._assert_manifest_document_failure_without_cargo(
            {"version": 1, "rows": [self._executable_manifest_row()], "attack": True},
            expected_substring="manifest root has unexpected keys: attack",
            git_sha="n5" + "0" * 38,
        )

    def test_unexpected_manifest_row_key_fails_closed_without_cargo(self) -> None:
        self._assert_manifest_document_failure_without_cargo(
            {
                "version": 1,
                "rows": [self._executable_manifest_row(attackerMetadata="drop-me")],
            },
            expected_substring="manifest rows[0] has unexpected keys: attackerMetadata",
            git_sha="n6" + "0" * 38,
        )

    def test_malformed_non_string_evidence_role_fails_closed_without_cargo(self) -> None:
        self._assert_schema_failure_without_cargo(
            [self._executable_manifest_row(evidenceRole=["primary"])],
            expected_substring="evidenceRole must be a string or omitted",
            git_sha="n7" + "0" * 38,
        )

    def test_malformed_non_string_coverage_note_fails_closed_without_cargo(self) -> None:
        self._assert_schema_failure_without_cargo(
            [self._executable_manifest_row(coverageNote={"note": True})],
            expected_substring="coverageNote must be a string or omitted",
            git_sha="n8" + "0" * 38,
        )

    def test_canonical_manifest_document_parses_and_validates(self) -> None:
        raw = json.loads(DEFAULT_MANIFEST.read_text(encoding="utf-8"))
        manifest, parse_errors = parse_manifest_document(raw)
        self.assertEqual(parse_errors, [], "\n".join(parse_errors))
        assert manifest is not None
        errors = validate_manifest_schema(manifest)
        self.assertEqual(errors, [], "\n".join(errors))

    def test_malformed_executable_non_string_binary_fails_closed_without_cargo(self) -> None:
        self._assert_schema_failure_without_cargo(
            [self._executable_manifest_row(id="denial-non-string-binary", binary=123)],
            expected_substring="binary must be a string or omitted",
            git_sha="k" * 40,
        )

    def test_malformed_executable_non_string_test_name_fails_closed_without_cargo(self) -> None:
        self._assert_schema_failure_without_cargo(
            [
                self._executable_manifest_row(
                    id="denial-non-string-test-name",
                    testName=["not", "a", "string"],
                )
            ],
            expected_substring="testName must be a string or omitted",
            git_sha="l" * 40,
        )

    def test_redact_report_dict_structured_access_token_has_no_json_residual(self) -> None:
        secret = "supersecret123456789"
        raw_report = {
            "gitShaFull": "m" * 40,
            "manifestSha256": "n" * 64,
            "failures": [],
            "findings": [],
            "leakageCount": 0,
            "redactionScan": {"passed": True, "findings": []},
            "tokenPayload": {"access_token": secret, "accessToken": secret},
        }
        redacted = redact_report_dict(raw_report)
        serialized = json.dumps(redacted, sort_keys=True)
        self.assertNotIn(secret, serialized)
        self.assertNotIn("supersecret", serialized.lower())
        self.assertEqual(redacted["tokenPayload"]["access_token"], STRUCTURED_REDACTED)
        self.assertEqual(redacted["tokenPayload"]["accessToken"], STRUCTURED_REDACTED)
        self.assertEqual(scan_for_secret_shapes(serialized), [])
        self.assertTrue(redacted.get("redactionScan", {}).get("passed", True))

    def test_malformed_executable_missing_binary_fails_closed_without_cargo(self) -> None:
        manifest_path = self.write_manifest(
            [
                {
                    "id": "denial-no-binary",
                    "operationId": "ask",
                    "guardInventoryRef": "ask",
                    "layer": "http",
                    "status": "executable",
                    "testName": "some_test",
                }
            ]
        )
        executor = FakeExecutor(
            list_outcomes={"some_binary": FakeExecutorOutcome(stdout="some_test: test\n")}
        )
        exit_code, report = run_suite(
            manifest_path=manifest_path,
            output_path=self.root / "report.json",
            repo_root=self.root,
            tests_root=self.tests_root,
            executor=executor,
            env=self.required_env,
            git_sha_resolver=lambda _root: "i" * 40,
        )
        self.assertEqual(exit_code, 2)
        assert report is not None
        self.assertTrue(
            any("denial-no-binary" in failure and "missing binary" in failure for failure in report.failures),
            report.failures,
        )
        self.assertEqual(executor.calls, [])
        payload = json.loads((self.root / "report.json").read_text(encoding="utf-8"))
        self.assertEqual(payload["gitShaFull"], "i" * 40)

    def test_malformed_executable_missing_test_name_fails_closed_without_cargo(self) -> None:
        self.write_source("api_http_contracts", ["live_http_retrieval_refuses_foreign_collection_scope"])
        manifest_path = self.write_manifest(
            [
                {
                    "id": "denial-no-test-name",
                    "binary": "api_http_contracts",
                    "operationId": "ask",
                    "guardInventoryRef": "ask",
                    "layer": "http",
                    "status": "executable",
                }
            ]
        )
        executor = FakeExecutor(
            list_outcomes={
                "api_http_contracts": FakeExecutorOutcome(
                    stdout="live_http_retrieval_refuses_foreign_collection_scope: test\n",
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
            git_sha_resolver=lambda _root: "j" * 40,
        )
        self.assertEqual(exit_code, 2)
        assert report is not None
        self.assertTrue(
            any("denial-no-test-name" in failure and "missing testName" in failure for failure in report.failures),
            report.failures,
        )
        self.assertEqual(executor.calls, [])

    def test_short_basic_auth_credential_redacted_in_emitted_failure_log(self) -> None:
        short_basic = "YTpi"
        child = ChildResult(
            binary="multi_org_denial",
            exit_code=101,
            stdout="",
            stderr=f"Authorization: Basic {short_basic}\n",
        )
        buffer = io.StringIO()
        emit_redacted_failure_output([child], fixture=None, stream=buffer)
        rendered = buffer.getvalue()
        self.assertNotIn(short_basic, rendered)
        self.assertIn("Authorization: Basic [REDACTED]", rendered)
        self.assertNotIn("<suppressed: residual secret shapes survived redaction>", rendered)
        self.assertEqual(scan_for_secret_shapes(rendered), [])

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
            list_outcomes={
                "api_http_contracts": FakeExecutorOutcome(
                    stdout="live_http_retrieval_refuses_foreign_collection_scope: test\n",
                )
            },
            outcomes={
                "api_http_contracts": FakeExecutorOutcome(
                    exit_code=101,
                    stdout="test live_http_retrieval_refuses_foreign_collection_scope ... FAILED\n",
                    stderr="assertion failed\n",
                )
            },
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
            list_outcomes={
                "multi_org_denial": FakeExecutorOutcome(
                    stdout="shared_world_http_surfaces_respect_org_scope: test\n",
                )
            },
            outcomes={
                "multi_org_denial": FakeExecutorOutcome(
                    stdout="leaked foreign marker phase1c-marker-beta in body\n",
                )
            },
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

    def test_failure_echo_is_redacted_bounded_and_skips_passing_children(self) -> None:
        import io

        fixture = DenialFixture(
            version=1,
            indexed_markers={"orgAlpha": "phase1c-marker-alpha"},
            duplicate_names={},
            object_key_template="denial/{orgKey}/{marker}.txt",
        )
        results = [
            ChildResult(binary="quiet_pass", exit_code=0, stdout="ok\n", stderr=""),
            ChildResult(
                binary="loud_fail",
                exit_code=101,
                stdout=(
                    "test fts_probe ... FAILED\n"
                    "panicked: marker phase1c-marker-alpha-abc123 missing\n"
                    "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.payload.signature\n"
                ),
                stderr="",
            ),
        ]
        stream = io.StringIO()
        emit_redacted_failure_output(results, fixture=fixture, stream=stream)
        echoed = stream.getvalue()
        self.assertIn("loud_fail", echoed)
        self.assertIn("FAILED", echoed)
        self.assertNotIn("quiet_pass", echoed)
        self.assertNotIn("phase1c-marker-alpha", echoed)
        self.assertNotIn("eyJhbGciOiJIUzI1NiJ9", echoed)

        oversized = ChildResult(
            binary="huge_fail",
            exit_code=101,
            stdout="x" * (FAILURE_ECHO_TAIL_CHARS * 3) + "\ntail marker line\n",
            stderr="",
        )
        stream = io.StringIO()
        emit_redacted_failure_output([oversized], fixture=fixture, stream=stream)
        echoed = stream.getvalue()
        self.assertIn("tail marker line", echoed)
        self.assertLess(len(echoed), FAILURE_ECHO_TAIL_CHARS + 500)

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

    def test_redacted_outputs_have_no_residual_secret_shape_labels(self) -> None:
        samples = {
            "assignment_access_token": "access_token=supersecret123456789",
            "assignment_secret": "secret=supersecret123456789",
            "assignment_api_key": "api_key=supersecret123456789",
            "markhand_env": "MARKHAND_TEST_MINIO_SECRET_KEY=markhand_app_poc_change_me",
            "ci_secret": "GITHUB_TOKEN=ghp_supersecret1234567890",
            "bearer": "Authorization: Bearer supersecret123456789",
            "jwt": (
                "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9."
                "eyJzdWIiOiIxMjM0NTY3ODkwIn0."
                "abcdefghijklmnopqrstuvwxyz1234567890"
            ),
            "database_url": "postgresql://markhand:markhand_poc_change_me@127.0.0.1:54330/markhand",
            "pem": (
                "-----BEGIN RSA PRIVATE KEY-----\n"
                "MIIEpAIBAAKCAQEAabc123\n"
                "-----END RSA PRIVATE KEY-----"
            ),
        }
        for label, raw in samples.items():
            with self.subTest(label=label):
                redacted = redact_text(raw)
                self.assertNotIn("supersecret", redacted.lower())
                self.assertNotIn("ghp_", redacted)
                self.assertNotIn("markhand_poc_change_me", redacted)
                self.assertNotIn("BEGIN RSA PRIVATE KEY", redacted)
                self.assertNotIn("eyJhbGci", redacted)
                residuals = scan_for_secret_shapes(redacted)
                self.assertEqual(
                    residuals,
                    [],
                    f"redacted output still matched secret shapes: {redacted!r}",
                )

    def test_emit_redacted_failure_output_redacts_secrets_without_suppression(self) -> None:
        secret = "supersecret123456789"
        child = ChildResult(
            binary="multi_org_denial",
            exit_code=101,
            stdout="",
            stderr=(
                "thread panicked\n"
                f"access_token={secret}\n"
                "MARKHAND_TEST_MINIO_SECRET_KEY=markhand_app_poc_change_me\n"
                f"GITHUB_TOKEN=ghp_{secret}\n"
            ),
        )
        buffer = io.StringIO()
        emit_redacted_failure_output([child], fixture=None, stream=buffer)
        rendered = buffer.getvalue()
        self.assertIn("redacted output tail: binary multi_org_denial", rendered)
        self.assertIn("[REDACTED]", rendered)
        self.assertNotIn(secret, rendered)
        self.assertNotIn("markhand_app_poc_change_me", rendered)
        self.assertNotIn("ghp_", rendered)
        self.assertNotIn("<suppressed: residual secret shapes survived redaction>", rendered)

    def test_redaction_covers_json_basic_cookie_and_camelcase_adversarial_shapes(
        self,
    ) -> None:
        secret = "supersecret123456789"
        samples = {
            "json_access_token": f'{{"access_token":"{secret}"}}',
            "json_refresh_token_camel": f'{{"refreshToken":"{secret}"}}',
            "basic_auth_header": "Authorization: Basic YTpi",
            "set_cookie_header": f"Set-Cookie: session={secret}; HttpOnly",
            "cookie_header": f"Cookie: sid={secret}; path=/",
            "camelcase_refresh_assignment": f"refreshToken={secret}",
        }
        for label, raw in samples.items():
            with self.subTest(label=label):
                redacted = redact_text(raw)
                self.assertNotIn(secret, redacted, redacted)
                residuals = scan_for_secret_shapes(redacted)
                self.assertEqual(
                    residuals,
                    [],
                    f"placeholder self-matched residual scan: {redacted!r} -> {residuals}",
                )

    def test_emit_redacted_failure_output_redacts_json_and_auth_headers(self) -> None:
        secret = "supersecret123456789"
        child = ChildResult(
            binary="multi_org_denial",
            exit_code=101,
            stdout=f'panic body {{"access_token":"{secret}"}}\n',
            stderr=(
                f"Authorization: Basic c3VwZXJzZWNy{secret}\n"
                f"Set-Cookie: session={secret}\n"
            ),
        )
        buffer = io.StringIO()
        emit_redacted_failure_output([child], fixture=None, stream=buffer)
        rendered = buffer.getvalue()
        self.assertNotIn(secret, rendered)
        self.assertNotIn("<suppressed: residual secret shapes survived redaction>", rendered)

    def test_emit_redacted_failure_output_suppresses_when_redaction_leaves_residual(
        self,
    ) -> None:
        leaked = "api_key=still_leaked_secret_value_123456789"
        child = ChildResult(
            binary="multi_org_denial",
            exit_code=101,
            stdout="",
            stderr=f"thread panicked\n{leaked}\n",
        )
        buffer = io.StringIO()
        with patch(f"{__name__}.redact_text", side_effect=lambda text, fixture=None: text):
            emit_redacted_failure_output([child], fixture=None, stream=buffer)
        rendered = buffer.getvalue()
        self.assertIn("<suppressed: residual secret shapes survived redaction>", rendered)
        self.assertNotIn("still_leaked_secret_value", rendered)
        self.assertNotIn(leaked, rendered)

    def test_redacted_failure_tail_echoes_diagnostic_output_when_clean(self) -> None:
        child = ChildResult(
            binary="multi_org_denial",
            exit_code=101,
            stdout="",
            stderr="thread 'test' panicked at crates/server/tests/multi_org_denial.rs:42:5\n",
        )
        buffer = io.StringIO()
        emit_redacted_failure_output([child], fixture=None, stream=buffer)
        rendered = buffer.getvalue()
        self.assertIn("redacted output tail: binary multi_org_denial", rendered)
        self.assertIn("panicked at crates/server/tests/multi_org_denial.rs", rendered)
        self.assertNotIn("<suppressed: residual secret shapes survived redaction>", rendered)

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
