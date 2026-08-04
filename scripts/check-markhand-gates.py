#!/usr/bin/env python3
"""Validate Markhand Web workload, hardware and decision-gate registry."""

from __future__ import annotations

import argparse
import errno
import hashlib
import json
import math
import os
import re
import stat
import subprocess
import sys
import tempfile
import unittest
import shutil
from collections.abc import Callable
from pathlib import Path
from unittest import mock


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_ROOT = ROOT / "bench/markhand_web"
PHASE1B_GATE_DIR = DEFAULT_ROOT / "reports/phase-1b-gate"
SECRET_PATTERNS = (
    re.compile(r"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----"),
    re.compile(r"\bAKIA[0-9A-Z]{16}\b"),
    re.compile(r"\bpostgres(?:ql)?://[^/\s:@]+:[^@\s/]+@"),
    re.compile(r"(?:^|\s)/(?:home|Users|workspace|tmp)/\S+"),
    re.compile(r"\b[A-Za-z]:\\Users\\"),
)
GATE_FAMILIES = {"G0-ARCH", "G0-RET", "G0-SEC", "G0-CAP", "G0-SLO", "G0-LIC", "G1C-SEC"}
PHASE1C_GATE_REPORT_SCHEMA = DEFAULT_ROOT / "schema/phase1c-gate-report.schema.json"
PHASE1C_ENVIRONMENT_ID = "phase1c-multi-org-poc"
PHASE1C_WORKLOAD_PROFILE_ID = "phase1c-multi-org"
PHASE1C_WORKLOAD_REF = "workloads.phase1c-multi-org"
PHASE1C_WORKLOAD_FILE = DEFAULT_ROOT / "workloads/phase1c-multi-org.yaml"
PHASE1C_ENVIRONMENT_FILE = DEFAULT_ROOT / "environments/phase1c-multi-org-poc.yaml"
PHASE1C_REPORT_DIR = DEFAULT_ROOT / "reports/phase-1c-gate"
PHASE1C_REPORT_FILE = PHASE1C_REPORT_DIR / "phase-1c-gate.json"
PHASE1C_REPORT_TEMPLATE = PHASE1C_REPORT_DIR / "phase-1c-gate.template.json"
PHASE1C_SLA_SOURCE = "docs/markhand-web-sla-targets.md"
PHASE1C_THRESHOLD_PROVENANCE = "docs/superpowers/plans/2026-07-31-phase1c-closure.md"
PHASE1C_WORKLOAD_SCENARIOS: tuple[str, ...] = (
    "multi_org_denial_replay",
    "membership_acl_revoke_bound",
    "quota_recovery_after_failure",
    "noisy_neighbor_fairness",
    "admin_mutation_audit_coverage",
    "worker_runtime_role_proof",
    "container_vulnerability_scan",
    "stale_token_isolation",
    "qdrant_partial_fail_closed",
)
PHASE1C_WORKLOAD_SEMANTICS: dict[str, object] = {
    "name": "phase1c-multi-org",
    "profileId": "phase1c-multi-org",
    "environmentId": PHASE1C_ENVIRONMENT_ID,
    "orgCount": 2,
    "embeddingProfile": "mock",
    "requiresDedicatedWorkerRole": True,
    "requiresWorkerDatabaseUrl": True,
    "durationSeconds": 3600,
    "scenarios": list(PHASE1C_WORKLOAD_SCENARIOS),
}
PHASE1C_WORKLOAD_PROFILE_BINDING: dict[str, str] = {
    "profileId": PHASE1C_WORKLOAD_PROFILE_ID,
    "environmentId": PHASE1C_ENVIRONMENT_ID,
    "sourcePath": "workloads/phase1c-multi-org.yaml",
}
G1C_COMMAND = "python3 bench/markhand_web/scripts/run_phase1c_gate.py"
G1C_GATE_FAMILY = "G1C-SEC"
PHASE1C_FAILURE_DISPOSITION = "block-phase-1c"
SHA256_HEX = re.compile(r"^[0-9a-f]{64}$")
GIT_SHA_HEX = re.compile(r"^[0-9a-f]{40}$")
ISO8601_Z_RE = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")
ALL_ZERO_SHA256 = "0" * 64
ALL_ZERO_GIT_SHA = "0" * 40
PHASE1C_METRIC_THRESHOLDS: dict[str, tuple[str, float | int]] = {
    "cross_tenant_leakage_count": ("==", 0),
    "post_commit_stale_authorizations": ("==", 0),
    "membership_acl_revoke_max_ms": ("<=", 3000),
    "quota_drift_after_recovery": ("==", 0),
    "quiet_org_query_p95_ms": ("<=", 500),
    "starvation_events": ("==", 0),
    "admin_mutation_audit_coverage_ratio": ("==", 1.0),
    "worker_dedicated_role_verified": ("==", 1),
    "undispositioned_high_critical_count": ("==", 0),
}
G1C_GATE_ROWS: tuple[dict[str, object], ...] = (
    {
        "id": "G1C-SEC-LEAKAGE",
        "metrics": ("cross_tenant_leakage_count",),
        "owner": "security-owner",
        "approver": "security-owner",
        "evidence": "bench/markhand_web/reports/phase-1c-gate/leakage.json",
        "notes": "Zero cross-tenant leakage under multi-org qualifying load on phase1c-multi-org-poc. Evidence not_run until Task 16 harness.",
    },
    {
        "id": "G1C-SEC-REVOKE",
        "metrics": ("membership_acl_revoke_max_ms",),
        "owner": "security-owner",
        "approver": "operations-owner",
        "evidence": "bench/markhand_web/reports/phase-1c-gate/revoke.json",
        "notes": "Membership/ACL revoke bound <=3000 ms on deployed POC. Evidence not_run until Task 16 harness.",
    },
    {
        "id": "G1C-SEC-ACL-CACHE",
        "metrics": ("post_commit_stale_authorizations",),
        "owner": "security-owner",
        "approver": "security-owner",
        "evidence": "bench/markhand_web/reports/phase-1c-gate/acl-cache.json",
        "notes": "Zero stale authorizations after ACL cache invalidation. Evidence not_run until Task 16 harness.",
    },
    {
        "id": "G1C-SEC-QUOTA-RECOVERY",
        "metrics": ("quota_drift_after_recovery",),
        "owner": "operations-owner",
        "approver": "operations-owner",
        "evidence": "bench/markhand_web/reports/phase-1c-gate/quota-recovery.json",
        "notes": "Zero quota drift after crash/retry/cancel recovery. Evidence not_run until Task 16 harness.",
    },
    {
        "id": "G1C-SEC-NOISY-NEIGHBOR",
        "metrics": ("quiet_org_query_p95_ms", "starvation_events"),
        "owner": "operations-owner",
        "approver": "operations-owner",
        "evidence": "bench/markhand_web/reports/phase-1c-gate/noisy-neighbor.json",
        "notes": "Quiet-org query P95 <=500 ms and zero starvation events under noisy-neighbor load. POC scope only; not Profile B scale. Evidence not_run until Task 16 harness.",
    },
    {
        "id": "G1C-SEC-AUDIT-COVERAGE",
        "metrics": ("admin_mutation_audit_coverage_ratio",),
        "owner": "security-owner",
        "approver": "security-owner",
        "evidence": "bench/markhand_web/reports/phase-1c-gate/audit-coverage.json",
        "notes": "Administrative mutation audit coverage ratio 1.0. Evidence not_run until Task 16 harness.",
    },
    {
        "id": "G1C-SEC-WORKER-ROLE",
        "metrics": ("worker_dedicated_role_verified",),
        "owner": "security-owner",
        "approver": "operations-owner",
        "evidence": "bench/markhand_web/reports/phase-1c-gate/worker-role.json",
        "notes": "Deployed worker runtime role markhand_worker with dedicated DB URL. Evidence not_run until Task 16 harness.",
    },
    {
        "id": "G1C-SEC-CONTAINER-VULNS",
        "metrics": ("undispositioned_high_critical_count",),
        "owner": "security-owner",
        "approver": "security-owner",
        "evidence": "bench/markhand_web/reports/phase-1c-gate/container-vulns.json",
        "notes": "Zero undispositioned high/critical container findings. Evidence not_run until Task 16 harness.",
    },
    {
        "id": "G1C-SEC-STALE-TOKENS",
        "metrics": ("post_commit_stale_authorizations",),
        "owner": "security-owner",
        "approver": "security-owner",
        "evidence": "bench/markhand_web/reports/phase-1c-gate/stale-tokens.json",
        "notes": "Token rotation/reuse/revoke isolation; zero stale authorization after revoke. Evidence not_run until Task 16 harness.",
    },
    {
        "id": "G1C-SEC-QDRANT-FAIL-CLOSED",
        "metrics": ("cross_tenant_leakage_count",),
        "owner": "security-owner",
        "approver": "security-owner",
        "evidence": "bench/markhand_web/reports/phase-1c-gate/qdrant-fail-closed.json",
        "notes": "Qdrant timeout/partial failure remains authz-safe with zero leakage. Evidence not_run until Task 16 harness.",
    },
)
PHASE1C_EVIDENCE_ALLOWLIST: frozenset[str] = frozenset(
    str(row["evidence"]) for row in G1C_GATE_ROWS
)
G1C_EXPECTED_IDS: frozenset[str] = frozenset(str(row["id"]) for row in G1C_GATE_ROWS)
OPERATORS = {">=", ">", "<=", "<", "=="}
FAILURE_DISPOSITIONS = {
    "block-phase-1b",
    "block-phase-1c",
    "block-phase-4",
    "block-issue",
    "research-only",
    "waive-with-adr",
}


def g1c_gate_metrics(row: dict[str, object]) -> tuple[str, ...]:
    metrics = row.get("metrics")
    if isinstance(metrics, (list, tuple)):
        return tuple(str(item) for item in metrics)
    metric = row.get("metric")
    if isinstance(metric, str):
        return (metric,)
    return ()


def g1c_metric_spec(metric: str) -> tuple[str, str]:
    operator, _value = PHASE1C_METRIC_THRESHOLDS[metric]
    if metric.endswith("_ratio"):
        return "ratio", "min"
    if metric.endswith("_ms"):
        return "milliseconds", "max"
    if metric == "worker_dedicated_role_verified":
        return "count", "min"
    return "count", "max" if operator in {"<=", "<", "=="} else "min"


def g1c_row_for_gate(gate_id: str) -> dict[str, object] | None:
    for row in G1C_GATE_ROWS:
        if row.get("id") == gate_id:
            return row
    return None


def g1c_metric_owners(metric: str) -> tuple[str, str]:
    for row in G1C_GATE_ROWS:
        if metric in g1c_gate_metrics(row):
            return str(row["owner"]), str(row["approver"])
    return "security-owner", "security-owner"


def g1c_metric_contract(metric: str) -> dict[str, object]:
    operator, value = PHASE1C_METRIC_THRESHOLDS[metric]
    unit, statistic = g1c_metric_spec(metric)
    owner, approver = g1c_metric_owners(metric)
    return {
        "name": metric,
        "unit": unit,
        "statistic": statistic,
        "threshold": {"operator": operator, "value": value},
        "owner": owner,
        "approver": approver,
    }


def g1c_gate_metric_contracts(gate: dict, row: dict[str, object]) -> tuple[list[dict[str, object]], list[str]]:
    """Return registry metric contracts without synthesizing missing metrics."""
    errors: list[str] = []
    gate_id = str(gate.get("id", "<missing>"))
    row_metrics = g1c_gate_metrics(row)
    extra = gate.get("metricContracts")

    if len(row_metrics) > 1:
        if not isinstance(extra, list) or not extra:
            errors.append(f"gate {gate_id}: metricContracts required for multi-metric gate")
            return [], errors
        contracts: list[dict[str, object]] = []
        for index, item in enumerate(extra):
            if not isinstance(item, dict):
                errors.append(f"gate {gate_id}: metricContracts[{index}] must be an object")
                continue
            contracts.append(item)
        return contracts, errors

    if isinstance(extra, list) and extra:
        if len(extra) != 1:
            errors.append(
                f"gate {gate_id}: single-metric gate must declare exactly one metricContracts item"
            )
        contracts = []
        for index, item in enumerate(extra):
            if not isinstance(item, dict):
                errors.append(f"gate {gate_id}: metricContracts[{index}] must be an object")
                continue
            contracts.append(item)
        return contracts, errors

    primary = row_metrics[0] if row_metrics else None
    if not isinstance(primary, str):
        errors.append(f"gate {gate_id}: missing primary metric")
        return [], errors
    metric = gate.get("metric") if isinstance(gate.get("metric"), dict) else {}
    return [
        {
            "name": primary,
            "unit": metric.get("unit"),
            "statistic": metric.get("statistic"),
            "threshold": gate.get("threshold") or {},
            "owner": gate.get("owner"),
            "approver": gate.get("approver"),
        }
    ], errors


def canonical_file_sha256(path: Path) -> tuple[str | None, str | None]:
    """Return (digest, error). Exactly one of digest or error is set."""
    if not path.exists():
        return None, f"missing canonical file {path}"
    if not path.is_file():
        return None, f"canonical file {path} is not a regular file"
    try:
        canonical = path.read_bytes().replace(b"\r\n", b"\n").replace(b"\r", b"\n")
    except OSError as error:
        return None, f"cannot read canonical file {path}: {error}"
    return hashlib.sha256(canonical).hexdigest(), None


def canonical_threshold_decisions_sha256() -> str:
    payload = json.dumps(canonical_threshold_decisions(), sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


def phase1c_canonical_fingerprints(
    root: Path,
    *,
    workspace_root: Path | None = None,
) -> tuple[dict[str, str], list[str]]:
    """Collect canonical Phase 1C file digests; never skip missing/unreadable files."""
    workspace = workspace_root or ROOT
    errors: list[str] = []
    fingerprints: dict[str, str] = {}
    files = {
        "environmentSha256": root / "environments/phase1c-multi-org-poc.yaml",
        "workloadSha256": root / "workloads/phase1c-multi-org.yaml",
        "gatesSha256": root / "gates.yaml",
        "slaSha256": workspace / PHASE1C_SLA_SOURCE,
    }
    for field, path in files.items():
        digest, error = canonical_file_sha256(path)
        if error:
            errors.append(error)
        elif digest is not None:
            fingerprints[field] = digest
    fingerprints["thresholdDecisionsSha256"] = canonical_threshold_decisions_sha256()
    return fingerprints, errors


def descriptor_flags_supported() -> bool:
    return hasattr(os, "O_NOFOLLOW") and hasattr(os, "O_DIRECTORY")


GitCommitVerifier = Callable[[Path, str], tuple[bool, str | None]]


def git_commit_resolves(repo_root: Path, commit: str) -> tuple[bool, str | None]:
    """Verify commit resolves to a git object without shell interpolation."""
    try:
        completed = subprocess.run(
            ["git", "-C", str(repo_root), "cat-file", "-e", f"{commit}^{{commit}}"],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as error:
        return False, f"git cat-file failed: {error}"
    if completed.returncode != 0:
        return False, f"git.commit {commit!r} does not resolve to a commit object"
    return True, None


PHASE1C_HARNESS_EXTENSION_KEYS = frozenset(
    {"markhandPhase1cGate", "embeddingProfile", "p1c8EvidenceMapping", "notes"}
)


def strip_phase1c_harness_extensions(report: dict) -> dict:
    return {key: value for key, value in report.items() if key not in PHASE1C_HARNESS_EXTENSION_KEYS}


def phase1c_evidence_qualification_errors(payload: object, *, context: str) -> list[str]:
    errors: list[str] = []
    if not isinstance(payload, dict):
        errors.append(f"{context}: evidence must be an object")
        return errors
    if payload.get("coverageLimited") is True or payload.get("coverageLimited"):
        errors.append(f"{context}: evidence coverageLimited blocks pass")
    if payload.get("metricsObserved") is not True:
        errors.append(f"{context}: evidence metricsObserved must be true for pass")
    return errors


def phase1c_evidence_metric_binding_errors(
    payload: object,
    *,
    context: str,
    report_metrics: dict[str, object] | None = None,
    gate_row: dict[str, object] | None = None,
) -> list[str]:
    errors: list[str] = []
    if not isinstance(payload, dict):
        return errors
    evidence_metrics = payload.get("metrics")
    if not isinstance(evidence_metrics, dict):
        errors.append(f"{context}: evidence metrics must be an object")
        return errors
    if gate_row is None:
        gate_id = payload.get("gateId")
        if isinstance(gate_id, str):
            gate_row = g1c_row_for_gate(gate_id)
    if gate_row is None:
        errors.append(f"{context}: unknown gate for evidence metric binding")
        return errors
    required_metrics = g1c_gate_metrics(gate_row)
    if not required_metrics:
        errors.append(f"{context}: gate has no qualifying metrics")
        return errors
    for metric in required_metrics:
        if metric not in evidence_metrics:
            errors.append(f"{context}: evidence missing metric {metric}")
            continue
        value = evidence_metrics[metric]
        if metric in PHASE1C_METRIC_THRESHOLDS:
            operator, limit = PHASE1C_METRIC_THRESHOLDS[metric]
            if not threshold_satisfied(value, operator, limit):
                errors.append(
                    f"{context}: evidence metric {metric}={value!r} violates threshold"
                )
        if isinstance(report_metrics, dict) and metric in report_metrics:
            if value != report_metrics[metric]:
                errors.append(
                    f"{context}: evidence metric {metric} must match report metrics.{metric}"
                )
    return errors


def phase1c_evidence_path_errors(
    repo_root: Path,
    evidence_path: object,
    *,
    context: str,
) -> list[str]:
    errors: list[str] = []
    if not isinstance(evidence_path, str) or not evidence_path.strip():
        errors.append(f"{context}: evidence path must be non-empty")
        return errors
    if evidence_path.startswith(("/", "\\")) or re.match(r"^[A-Za-z]:\\", evidence_path):
        errors.append(f"{context}: evidence path must be relative")
        return errors
    if ".." in Path(evidence_path).parts:
        errors.append(f"{context}: evidence path must not contain traversal")
        return errors
    if evidence_path not in PHASE1C_EVIDENCE_ALLOWLIST:
        errors.append(f"{context}: evidence path not in canonical allowlist")
        return errors
    if not descriptor_flags_supported():
        errors.append(
            f"{context}: platform lacks O_NOFOLLOW/O_DIRECTORY for descriptor evidence validation"
        )
        return errors

    parts = Path(evidence_path).parts
    dir_fd: int | None = None
    file_fd: int | None = None
    try:
        dir_fd = os.open(
            str(repo_root.resolve()),
            os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
        )
        for part in parts[:-1]:
            next_fd = os.open(
                part,
                os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                dir_fd=dir_fd,
            )
            os.close(dir_fd)
            dir_fd = next_fd
        file_fd = os.open(parts[-1], os.O_RDONLY | os.O_NOFOLLOW, dir_fd=dir_fd)
        file_stat = os.fstat(file_fd)
        if stat.S_ISLNK(file_stat.st_mode):
            errors.append(f"{context}: evidence path must not be a symlink (descriptor/no-follow)")
            return errors
        if stat.S_ISDIR(file_stat.st_mode):
            errors.append(f"{context}: evidence path must be a regular file, not a directory")
            return errors
        if not stat.S_ISREG(file_stat.st_mode):
            errors.append(f"{context}: evidence path must be a regular file")
            return errors
        if file_stat.st_size == 0:
            errors.append(f"{context}: evidence file must not be empty")
    except OSError as error:
        if error.errno == errno.ELOOP:
            errors.append(
                f"{context}: evidence path must not contain a symlink component (O_NOFOLLOW)"
            )
        elif error.errno == errno.ENOTDIR:
            errors.append(f"{context}: evidence path component is not a directory")
        elif error.errno == errno.ENOENT:
            errors.append(f"{context}: evidence file missing at {evidence_path}")
        else:
            errors.append(f"{context}: descriptor evidence open failed: {error}")
    finally:
        if file_fd is not None:
            try:
                os.close(file_fd)
            except OSError:
                pass
        if dir_fd is not None:
            try:
                os.close(dir_fd)
            except OSError:
                pass
    return errors


def phase1c_workload_semantics_errors(profile: dict, *, source: str) -> list[str]:
    errors: list[str] = []
    for field, expected in PHASE1C_WORKLOAD_SEMANTICS.items():
        actual = profile.get(field)
        if field == "scenarios":
            if not isinstance(actual, list) or list(actual) != list(expected):
                errors.append(f"{source}: scenarios must match canonical Phase 1C workload")
            continue
        if actual != expected:
            errors.append(f"{source}: {field} must be {expected!r}")
    return errors


def is_valid_sha256(value: object, *, reject_all_zero: bool = True) -> bool:
    if not isinstance(value, str) or not SHA256_HEX.fullmatch(value):
        return False
    return not reject_all_zero or value != ALL_ZERO_SHA256


def is_valid_git_sha(value: object, *, reject_all_zero: bool = True) -> bool:
    if not isinstance(value, str) or not GIT_SHA_HEX.fullmatch(value):
        return False
    return not reject_all_zero or value != ALL_ZERO_GIT_SHA


def is_valid_iso8601_z(value: object) -> bool:
    return isinstance(value, str) and bool(ISO8601_Z_RE.fullmatch(value))


def threshold_satisfied(value: object, operator: str, limit: float | int) -> bool:
    if not numeric(value):
        return False
    if operator == "==":
        return value == limit
    if operator == ">=":
        return value >= limit
    if operator == ">":
        return value > limit
    if operator == "<=":
        return value <= limit
    if operator == "<":
        return value < limit
    return False


def canonical_threshold_decisions() -> list[dict[str, object]]:
    decisions: list[dict[str, object]] = []
    for metric in PHASE1C_METRIC_THRESHOLDS:
        contract = g1c_metric_contract(metric)
        threshold = contract["threshold"]
        if not isinstance(threshold, dict):
            continue
        decisions.append(
            {
                "metric": metric,
                "operator": threshold["operator"],
                "value": threshold["value"],
                "unit": contract["unit"],
                "statistic": contract["statistic"],
                "source": PHASE1C_SLA_SOURCE,
                "provenance": PHASE1C_THRESHOLD_PROVENANCE,
                "provenanceKind": "repository-design-decision",
                "owner": contract["owner"],
                "approver": contract["approver"],
                "recordedAt": "2026-08-04T00:00:00Z",
            }
        )
    return decisions


SCALE_FIELDS = (
    "orgCount",
    "collectionsPerOrg",
    "documentsPerCollection",
    "averageDocumentPages",
    "vectorsPerOrgMax",
    "aggregateVectors",
)
LOAD_FIELDS = {
    "normal": ("concurrentQueries", "ingestDocumentsPerHour", "deleteOperationsPerHour"),
    "peak": ("concurrentQueries", "ingestDocumentsPerHour", "deleteOperationsPerHour"),
    "recovery": ("loadMultiplier", "durationMinutes"),
    "aggregate": ("concurrentQueries", "concurrentIngest"),
}
SERVICE_TARGET_FIELDS = (
    "bestModelNdcgGapMax",
    "filteredQueryP99Ms",
    "temporalAccuracyMin",
    "changeAccuracyMin",
    "versionCitationPrecisionMin",
    "versionCitationRecallMin",
    "rpoMinutes",
    "queryReadyRtoMinutes",
    "fullVectorRtoMinutes",
)


def load_json_yaml(path: Path) -> dict:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as error:
        raise ValueError(f"{path}: YAML must remain JSON-compatible: {error}") from error


def has_null(value: object) -> bool:
    if value is None:
        return True
    if isinstance(value, dict):
        return any(has_null(item) for item in value.values())
    if isinstance(value, list):
        return any(has_null(item) for item in value)
    return False


def schema_errors(value: object, schema: dict, path: str) -> list[str]:
    errors: list[str] = []
    expected = schema.get("type")
    allowed = expected if isinstance(expected, list) else [expected] if expected else []
    type_matches = {
        "object": lambda item: isinstance(item, dict),
        "array": lambda item: isinstance(item, list),
        "string": lambda item: isinstance(item, str),
        "number": numeric,
        "integer": lambda item: isinstance(item, int) and not isinstance(item, bool),
        "boolean": lambda item: isinstance(item, bool),
        "null": lambda item: item is None,
    }
    if allowed and not any(type_matches[kind](value) for kind in allowed):
        return [f"{path}: schema type must be {'/'.join(allowed)}"]
    if "const" in schema and value != schema["const"]:
        errors.append(f"{path}: schema const mismatch")
    if "enum" in schema and value not in schema["enum"]:
        errors.append(f"{path}: value is outside schema enum")
    if isinstance(value, str):
        if len(value) < schema.get("minLength", 0):
            errors.append(f"{path}: string is shorter than schema minLength")
        if schema.get("pattern") and not re.fullmatch(schema["pattern"], value):
            errors.append(f"{path}: string does not match schema pattern")
        if schema.get("format") == "date-time" and not is_valid_iso8601_z(value):
            errors.append(f"{path}: string must be ISO8601 date-time")
    if numeric(value):
        if "minimum" in schema and value < schema["minimum"]:
            errors.append(f"{path}: number is below schema minimum")
        if "maximum" in schema and value > schema["maximum"]:
            errors.append(f"{path}: number is above schema maximum")
        if "exclusiveMinimum" in schema and value <= schema["exclusiveMinimum"]:
            errors.append(f"{path}: number is below schema exclusiveMinimum")
        if "exclusiveMaximum" in schema and value >= schema["exclusiveMaximum"]:
            errors.append(f"{path}: number is at or above schema exclusiveMaximum")
    elif isinstance(value, bool) and (
        "minimum" in schema or "maximum" in schema or "exclusiveMinimum" in schema or "exclusiveMaximum" in schema
    ):
        errors.append(f"{path}: boolean is not a schema number")
    if isinstance(value, dict):
        if len(value) < schema.get("minProperties", 0):
            errors.append(f"{path}: object has too few properties")
        for field in schema.get("required", []):
            if field not in value:
                errors.append(f"{path}: schema missing required field {field}")
        if schema.get("additionalProperties") is False:
            allowed = set(schema.get("properties", {}).keys())
            for field in value:
                if field not in allowed:
                    errors.append(f"{path}: unknown property {field}")
        for field, child_schema in schema.get("properties", {}).items():
            if field in value:
                errors.extend(schema_errors(value[field], child_schema, f"{path}.{field}"))
    if isinstance(value, list):
        if len(value) < schema.get("minItems", 0):
            errors.append(f"{path}: array has too few items")
        max_items = schema.get("maxItems")
        if isinstance(max_items, int) and len(value) > max_items:
            errors.append(f"{path}: array has too many items")
        if isinstance(schema.get("items"), dict):
            for index, item in enumerate(value):
                errors.extend(schema_errors(item, schema["items"], f"{path}[{index}]"))
    return errors


def dot_path(value: dict, path: str) -> bool:
    current: object = value
    for part in path.split("."):
        if not isinstance(current, dict) or part not in current:
            return False
        current = current[part]
    return True


def required_fields(value: dict, fields: tuple[str, ...], source: str) -> list[str]:
    return [f"{source}: missing {field}" for field in fields if field not in value]


def positive_number(value: object) -> bool:
    return numeric(value) and value > 0


def non_negative_number(value: object) -> bool:
    return numeric(value) and value >= 0


def numeric(value: object) -> bool:
    return (
        isinstance(value, (int, float))
        and not isinstance(value, bool)
        and math.isfinite(value)
    )


def security_errors(paths: list[Path]) -> list[str]:
    errors = []
    for path in paths:
        content = path.read_text(encoding="utf-8")
        if any(pattern.search(content) for pattern in SECRET_PATTERNS):
            errors.append(f"{path}: secret, credential, or absolute machine path detected")
    return errors


def validate(root: Path) -> list[str]:
    workload_path = root / "workload-profile.yaml"
    gates_path = root / "gates.yaml"
    environment_paths = sorted((root / "environments").glob("*.yaml"))
    workload = load_json_yaml(workload_path)
    registry = load_json_yaml(gates_path)
    environments = [load_json_yaml(path) for path in environment_paths]
    workload_schema = load_json_yaml(root / "schema/workload-profile.schema.json")
    gates_schema = load_json_yaml(root / "schema/gates.schema.json")
    environment_schema = load_json_yaml(root / "schema/environment.schema.json")
    errors = security_errors([workload_path, gates_path, *environment_paths])
    errors += schema_errors(workload, workload_schema, "workload")
    errors += schema_errors(registry, gates_schema, "gates")
    for environment, path in zip(environments, environment_paths):
        errors += schema_errors(environment, environment_schema, f"environment {path.name}")

    errors += required_fields(
        workload,
        (
            "version", "profileId", "status", "approver", "approvedAt",
            "openDecisions", "scale", "loads", "workloads", "serviceTargets",
            "hardware",
        ),
        "workload",
    )
    if workload.get("version") != 1:
        errors.append("workload: version must be 1")
    if workload.get("status") not in {"proposed", "approved"}:
        errors.append("workload: invalid status")
    if not isinstance(workload.get("approver"), str) or not workload["approver"].strip():
        errors.append("workload: approver must be non-empty")
    for tier in ("normal", "peak", "recovery", "aggregate"):
        if not isinstance(workload.get("loads", {}).get(tier), dict):
            errors.append(f"workload: missing loads.{tier}")
    for decision in workload.get("openDecisions", []):
        errors += required_fields(
            decision,
            ("id", "question", "owner", "status", "blocks"),
            "decision",
        )
        if decision.get("status") == "open" and not decision.get("owner"):
            errors.append(f"decision {decision.get('id')}: open decision requires owner")
        if decision.get("status") == "resolved" and not str(decision.get("resolution", "")).strip():
            errors.append(f"decision {decision.get('id')}: resolved decision requires resolution")
        if not isinstance(decision.get("blocks"), list) or not decision.get("blocks"):
            errors.append(f"decision {decision.get('id')}: blocks must be non-empty")
    if workload.get("status") == "approved":
        if not workload.get("approvedAt") or has_null(workload.get("scale")) or has_null(workload.get("loads")):
            errors.append("workload: approved profile requires approvedAt and complete scale/load values")
        unresolved = [
            decision.get("id")
            for decision in workload.get("openDecisions", [])
            if decision.get("status") != "resolved"
        ]
        if unresolved:
            errors.append(f"workload: approved profile has unresolved decisions {unresolved}")
        scale = workload.get("scale", {})
        for field in SCALE_FIELDS:
            if not positive_number(scale.get(field)):
                errors.append(f"workload: approved scale.{field} must be positive")
        loads = workload.get("loads", {})
        for tier, fields in LOAD_FIELDS.items():
            for field in fields:
                if not positive_number(loads.get(tier, {}).get(field)):
                    errors.append(f"workload: approved loads.{tier}.{field} must be positive")
        normal = loads.get("normal", {})
        peak = loads.get("peak", {})
        for field in LOAD_FIELDS["normal"]:
            if positive_number(normal.get(field)) and positive_number(peak.get(field)):
                if peak[field] < normal[field]:
                    errors.append(f"workload: peak {field} must be >= normal")
        tenant_distribution = loads.get("aggregate", {}).get("tenantDistribution")
        if not isinstance(tenant_distribution, str) or not tenant_distribution.strip():
            errors.append("workload: approved tenantDistribution must be non-empty")
        service_targets = workload.get("serviceTargets", {})
        for field in SERVICE_TARGET_FIELDS:
            if not positive_number(service_targets.get(field)):
                errors.append(f"workload: approved serviceTargets.{field} must be positive")
        for field in (
            "temporalAccuracyMin",
            "changeAccuracyMin",
            "versionCitationPrecisionMin",
            "versionCitationRecallMin",
        ):
            if numeric(service_targets.get(field)) and service_targets[field] > 1:
                errors.append(f"workload: approved serviceTargets.{field} must be <=1")
        headroom = workload.get("hardware", {}).get("headroomPercent", {})
        for resource in ("cpu", "ram", "disk"):
            value = headroom.get(resource)
            if not positive_number(value) or value >= 100:
                errors.append(
                    f"workload: approved headroomPercent.{resource} must be within 0..100"
                )

    environment_ids: set[str] = set()
    for environment, path in zip(environments, environment_paths):
        source = f"environment {path.name}"
        errors += required_fields(
            environment,
            ("version", "environmentId", "role", "status", "approver", "cpu", "ramGb", "disk", "gpu", "network", "os", "fingerprintRequiredFields"),
            source,
        )
        environment_id = environment.get("environmentId")
        if environment_id in environment_ids:
            errors.append(f"{source}: duplicate environmentId {environment_id}")
        if isinstance(environment_id, str):
            environment_ids.add(environment_id)
        if environment.get("status") == "approved":
            if not isinstance(environment.get("approver"), str) or not environment["approver"].strip():
                errors.append(f"{source}: approved environment requires non-empty approver")
            if has_null(
                {key: environment.get(key) for key in ("cpu", "ramGb", "disk", "gpu", "network")}
            ):
                errors.append(f"{source}: approved environment has null hardware values")
            if not environment.get("approvedAt"):
                errors.append(f"{source}: approved environment requires approvedAt")
            numeric_paths = (
                ("cpu.cores", environment.get("cpu", {}).get("cores")),
                ("cpu.threads", environment.get("cpu", {}).get("threads")),
                ("ramGb", environment.get("ramGb")),
                ("disk.capacityGb", environment.get("disk", {}).get("capacityGb")),
                ("network.bandwidthGbps", environment.get("network", {}).get("bandwidthGbps")),
                ("network.latencyMsAssumed", environment.get("network", {}).get("latencyMsAssumed")),
            )
            for field, value in numeric_paths:
                if not positive_number(value):
                    errors.append(f"{source}: approved {field} must be positive")
            # CPU-only quality environments may honestly declare gpu.count=0 / vramGb=0.
            gpu = environment.get("gpu") or {}
            gpu_count = gpu.get("count")
            gpu_vram = gpu.get("vramGb")
            if not non_negative_number(gpu_count):
                errors.append(f"{source}: approved gpu.count must be >= 0")
            if not non_negative_number(gpu_vram):
                errors.append(f"{source}: approved gpu.vramGb must be >= 0")
            if non_negative_number(gpu_count) and non_negative_number(gpu_vram):
                if gpu_count == 0 and gpu_vram != 0:
                    errors.append(
                        f"{source}: approved gpu.vramGb must be 0 when gpu.count is 0"
                    )
                if gpu_count > 0 and gpu_vram <= 0:
                    errors.append(
                        f"{source}: approved gpu.vramGb must be positive when gpu.count > 0"
                    )
            string_paths = (
                ("cpu.vendor", environment.get("cpu", {}).get("vendor")),
                ("cpu.model", environment.get("cpu", {}).get("model")),
                ("disk.type", environment.get("disk", {}).get("type")),
                ("disk.iopsNote", environment.get("disk", {}).get("iopsNote")),
                ("gpu.model", environment.get("gpu", {}).get("model")),
                ("os.distro", environment.get("os", {}).get("distro")),
                ("os.arch", environment.get("os", {}).get("arch")),
            )
            for field, value in string_paths:
                if not isinstance(value, str) or not value.strip():
                    errors.append(f"{source}: approved {field} must be non-empty")
            fingerprint_fields = environment.get("fingerprintRequiredFields")
            if (
                not isinstance(fingerprint_fields, list)
                or not fingerprint_fields
                or len(set(fingerprint_fields)) != len(fingerprint_fields)
                or any(not isinstance(field, str) or not field.strip() for field in fingerprint_fields)
            ):
                errors.append(f"{source}: fingerprintRequiredFields must be unique non-empty strings")
        if environment_id == PHASE1C_ENVIRONMENT_ID and environment.get("status") == "approved":
            for field, expected in (
                ("orgCount", 2),
                ("embeddingProfile", "mock"),
                ("requiresDedicatedWorkerRole", True),
                ("requiresWorkerDatabaseUrl", True),
            ):
                if environment.get(field) != expected:
                    errors.append(f"{source}: {field} must be {expected!r}")

    workload_environment = workload.get("hardware", {}).get("environmentId")
    if workload_environment not in environment_ids:
        errors.append(f"workload: unknown environmentId {workload_environment}")

    errors += required_fields(registry, ("version", "registryStatus", "gates"), "gates")
    if registry.get("registryStatus") not in {"proposed", "approved", "closed"}:
        errors.append("gates: invalid registryStatus")
    ids: set[str] = set()
    families: set[str] = set()
    for gate in registry.get("gates", []):
        gate_id = gate.get("id", "<missing>")
        errors += required_fields(
            gate,
            ("id", "externalGate", "metric", "workload", "threshold", "command", "environmentId", "owner", "approver", "status", "failureDisposition"),
            f"gate {gate_id}",
        )
        if gate_id in ids:
            errors.append(f"duplicate gate id: {gate_id}")
        ids.add(gate_id)
        family = gate.get("externalGate")
        families.add(family)
        if family not in GATE_FAMILIES:
            errors.append(f"gate {gate_id}: invalid externalGate")
        if gate.get("environmentId") not in environment_ids:
            errors.append(f"gate {gate_id}: unknown environmentId")
        workload_ref = gate.get("workload")
        if not isinstance(workload_ref, str) or not dot_path(workload, workload_ref):
            errors.append(f"gate {gate_id}: workload path does not resolve")
        if str(gate.get("id", "")).startswith("G1C-SEC-") and workload_ref != PHASE1C_WORKLOAD_REF:
            errors.append(f"gate {gate_id}: G1C workload must be {PHASE1C_WORKLOAD_REF!r}")
        threshold = gate.get("threshold", {})
        if threshold.get("operator") not in OPERATORS:
            errors.append(f"gate {gate_id}: invalid threshold operator")
        status = gate.get("status")
        if status not in {"proposed", "approved", "measured", "failed", "waived"}:
            errors.append(f"gate {gate_id}: invalid status")
        if status != "proposed" and not numeric(threshold.get("value")):
            errors.append(f"gate {gate_id}: non-proposed threshold must be numeric")
        metric = gate.get("metric", {})
        for field in ("name", "unit", "statistic"):
            if not isinstance(metric.get(field), str) or not metric[field].strip():
                errors.append(f"gate {gate_id}: metric.{field} must be non-empty")
        if (
            metric.get("unit") == "ratio"
            and numeric(threshold.get("value"))
            and not 0 <= threshold["value"] <= 1
        ):
            errors.append(f"gate {gate_id}: ratio threshold must be within 0..1")
        if gate.get("failureDisposition") not in FAILURE_DISPOSITIONS:
            errors.append(f"gate {gate_id}: invalid failureDisposition")
        for field in ("owner", "approver", "command"):
            if not isinstance(gate.get(field), str) or not gate[field].strip():
                errors.append(f"gate {gate_id}: {field} must be non-empty")
    missing_families = GATE_FAMILIES - families
    if missing_families:
        errors.append(f"gates: missing external families {sorted(missing_families)}")
    if registry.get("registryStatus") == "approved":
        not_approved = [
            gate.get("id")
            for gate in registry.get("gates", [])
            if gate.get("status") != "approved"
        ]
        if not_approved:
            errors.append(f"gates: approved registry has non-approved gates {not_approved}")
        if workload.get("status") != "approved":
            errors.append("gates: approved registry requires approved workload")
        environment_status = {
            environment.get("environmentId"): environment.get("status")
            for environment in environments
        }
        unapproved_environments = sorted(
            {
                gate.get("environmentId")
                for gate in registry.get("gates", [])
                if environment_status.get(gate.get("environmentId")) != "approved"
            },
            key=str,
        )
        if unapproved_environments:
            errors.append(
                f"gates: approved registry uses unapproved environments {unapproved_environments}"
            )
        gate_by_id = {gate.get("id"): gate for gate in registry.get("gates", [])}
        expected_thresholds = {
            "G0-RET-BEST-MODEL-GAP": (
                "<=",
                workload.get("serviceTargets", {}).get("bestModelNdcgGapMax"),
            ),
            "G0-SLO-QUERY-P99": (
                "<=",
                workload.get("serviceTargets", {}).get("filteredQueryP99Ms"),
            ),
            "G0-RET-TEMPORAL-ACCURACY": (
                ">=",
                workload.get("serviceTargets", {}).get("temporalAccuracyMin"),
            ),
            "G0-RET-CHANGE-ACCURACY": (
                ">=",
                workload.get("serviceTargets", {}).get("changeAccuracyMin"),
            ),
            "G0-RET-VERSION-CITATION-PRECISION": (
                ">=",
                workload.get("serviceTargets", {}).get(
                    "versionCitationPrecisionMin"
                ),
            ),
            "G0-RET-VERSION-CITATION-RECALL": (
                ">=",
                workload.get("serviceTargets", {}).get("versionCitationRecallMin"),
            ),
            "G0-DR-RPO": (
                "<=",
                workload.get("serviceTargets", {}).get("rpoMinutes"),
            ),
            "G0-DR-QUERY-READY-RTO": (
                "<=",
                workload.get("serviceTargets", {}).get("queryReadyRtoMinutes"),
            ),
            "G0-DR-FULL-VECTOR-RTO": (
                "<=",
                workload.get("serviceTargets", {}).get("fullVectorRtoMinutes"),
            ),
            "G0-CAP-INGEST-THROUGHPUT": (
                ">=",
                workload.get("loads", {}).get("peak", {}).get("ingestDocumentsPerHour"),
            ),
        }
        for gate_id, (operator, value) in expected_thresholds.items():
            gate = gate_by_id.get(gate_id)
            threshold = gate.get("threshold", {}) if gate else {}
            if not gate or threshold.get("operator") != operator or threshold.get("value") != value:
                errors.append(f"gates: {gate_id} diverges from approved workload target")
    errors += phase1c_registry_contract_errors(registry, environments, root=root)
    return errors


_MD_STATUS_RE = re.compile(r"^Status: \*\*(.*?)\*\*\s*$", re.MULTILINE)


def phase1b_gate_report_errors(gate_dir: Path) -> list[str]:
    """Fail closed if summary.json / phase-1b-gate.md disagree with the
    canonical o05-soak.json inside gate_dir.

    All three files are written by the same function
    (bench/markhand_web/soak/report.py:write_reports, issue P1B-O05); if a
    passing run's canonical o05-soak.json gets committed but the derived
    summary.json / phase-1b-gate.md do not, a machine reading gates.yaml
    (which points at o05-soak.json) sees "pass" while a human opening
    summary.json or phase-1b-gate.md still sees the stale status. This
    happened once (o05-soak.json from PR #309 vs summary.json/
    phase-1b-gate.md still from PR #306) — catch it mechanically instead of
    relying on someone noticing.
    """
    errors: list[str] = []
    canonical_path = gate_dir / "o05-soak.json"
    summary_path = gate_dir / "summary.json"
    md_path = gate_dir / "phase-1b-gate.md"

    try:
        canonical = load_json_yaml(canonical_path)
    except (OSError, ValueError) as error:
        return [f"{canonical_path}: cannot read canonical o05-soak.json: {error}"]
    if not isinstance(canonical, dict):
        return [f"{canonical_path}: canonical o05-soak.json is not an object"]

    canonical_status = canonical.get("status")
    canonical_git = (canonical.get("versions") or {}).get("git")
    canonical_blockers = canonical.get("blockers") or []

    try:
        summary = load_json_yaml(summary_path)
    except (OSError, ValueError) as error:
        errors.append(f"{summary_path}: cannot read summary.json: {error}")
        summary = None
    if isinstance(summary, dict):
        summary_status = summary.get("status")
        summary_git = (summary.get("versions") or {}).get("git")
        summary_blockers = summary.get("blockers") or []
        if summary_status != canonical_status:
            errors.append(
                f"{summary_path}: status {summary_status!r} disagrees with "
                f"canonical {canonical_path.name} status {canonical_status!r}"
            )
        if summary_git != canonical_git:
            errors.append(
                f"{summary_path}: versions.git {summary_git!r} disagrees with "
                f"canonical {canonical_path.name} versions.git {canonical_git!r}"
            )
        if summary_blockers != canonical_blockers:
            errors.append(
                f"{summary_path}: blockers disagree with canonical {canonical_path.name}"
            )
    elif summary is not None:
        errors.append(f"{summary_path}: summary.json is not an object")

    try:
        md_text = md_path.read_text(encoding="utf-8")
    except OSError as error:
        errors.append(f"{md_path}: cannot read phase-1b-gate.md: {error}")
        md_text = None
    if md_text is not None:
        match = _MD_STATUS_RE.search(md_text)
        md_status = match.group(1) if match else None
        if md_status != canonical_status:
            errors.append(
                f"{md_path}: status line {md_status!r} disagrees with "
                f"canonical {canonical_path.name} status {canonical_status!r}"
            )

    return errors


def phase1c_registry_contract_errors(
    registry: dict,
    environments: list[dict],
    *,
    root: Path | None = None,
    workspace_root: Path | None = None,
    require_g1c_rows: bool = True,
) -> list[str]:
    """Validate Phase 1C G1C-SEC registry rows and qualifying environment binding."""
    workspace = workspace_root or ROOT
    errors: list[str] = []
    if not require_g1c_rows:
        return errors

    if registry.get("registryStatus") != "approved":
        errors.append("gates: Phase 1C contract requires registryStatus approved")

    gates = registry.get("gates")
    if not isinstance(gates, list):
        return errors + ["gates: gates must be an array"]

    env_by_id = {
        environment.get("environmentId"): environment
        for environment in environments
        if isinstance(environment, dict) and isinstance(environment.get("environmentId"), str)
    }
    phase1c_env = env_by_id.get(PHASE1C_ENVIRONMENT_ID)
    if phase1c_env is None:
        errors.append(f"gates: missing qualifying environment {PHASE1C_ENVIRONMENT_ID}")
    else:
        if phase1c_env.get("status") != "approved":
            errors.append(f"environment {PHASE1C_ENVIRONMENT_ID}: status must be approved")
        for field, expected in (
            ("orgCount", 2),
            ("embeddingProfile", "mock"),
            ("requiresDedicatedWorkerRole", True),
            ("requiresWorkerDatabaseUrl", True),
        ):
            if phase1c_env.get(field) != expected:
                errors.append(
                    f"environment {PHASE1C_ENVIRONMENT_ID}: {field} must be {expected!r}"
                )

    if root is not None:
        for label, rel in (
            ("environment", "environments/phase1c-multi-org-poc.yaml"),
            ("workload", "workloads/phase1c-multi-org.yaml"),
            ("gates", "gates.yaml"),
        ):
            _digest, file_error = canonical_file_sha256(root / rel)
            if file_error:
                errors.append(file_error)
        sla_digest, sla_error = canonical_file_sha256(workspace / PHASE1C_SLA_SOURCE)
        if sla_error:
            errors.append(sla_error)

        workload_profile = root / "workloads/phase1c-multi-org.yaml"
        if not workload_profile.is_file():
            errors.append(f"gates: missing workload profile {workload_profile}")
        else:
            try:
                profile = load_json_yaml(workload_profile)
            except ValueError as error:
                errors.append(str(error))
                profile = None
            if isinstance(profile, dict):
                errors += phase1c_workload_semantics_errors(
                    profile,
                    source="workloads/phase1c-multi-org.yaml",
                )

        try:
            workload_registry = load_json_yaml(root / "workload-profile.yaml")
        except (OSError, ValueError) as error:
            errors.append(f"gates: cannot read workload-profile.yaml: {error}")
            workload_registry = None
        if isinstance(workload_registry, dict):
            workloads = workload_registry.get("workloads")
            if not isinstance(workloads, dict):
                errors.append("workload-profile.yaml: workloads must be an object")
            else:
                binding = workloads.get(PHASE1C_WORKLOAD_PROFILE_ID)
                if not isinstance(binding, dict):
                    errors.append(
                        f"workload-profile.yaml: missing workloads.{PHASE1C_WORKLOAD_PROFILE_ID} binding"
                    )
                else:
                    for field, expected in PHASE1C_WORKLOAD_PROFILE_BINDING.items():
                        if binding.get(field) != expected:
                            errors.append(
                                f"workload-profile.yaml: workloads.{PHASE1C_WORKLOAD_PROFILE_ID}."
                                f"{field} must be {expected!r}"
                            )

    g1c_gates = [
        gate
        for gate in gates
        if isinstance(gate, dict) and gate.get("id") in G1C_EXPECTED_IDS
    ]
    expected_ids = set(G1C_EXPECTED_IDS)
    found_ids = {gate.get("id") for gate in g1c_gates}
    for gate in gates:
        if not isinstance(gate, dict):
            continue
        gate_id = str(gate.get("id", ""))
        family = gate.get("externalGate")
        if family == G1C_GATE_FAMILY and gate_id not in G1C_EXPECTED_IDS:
            errors.append(
                f"gate {gate_id}: externalGate {G1C_GATE_FAMILY} requires canonical G1C id"
            )
        if gate_id in G1C_EXPECTED_IDS and family != G1C_GATE_FAMILY:
            errors.append(
                f"gate {gate_id}: canonical G1C id requires externalGate {G1C_GATE_FAMILY}"
            )
    missing_ids = sorted(expected_ids - found_ids)
    extra_ids = sorted(found_ids - expected_ids)
    if missing_ids:
        errors.append(f"gates: missing G1C-SEC rows {missing_ids}")
    if extra_ids:
        errors.append(f"gates: unexpected G1C-SEC rows {extra_ids}")
    if len(g1c_gates) != len(G1C_GATE_ROWS):
        errors.append(
            f"gates: expected {len(G1C_GATE_ROWS)} G1C-SEC rows, found {len(g1c_gates)}"
        )

    if not any(
        isinstance(gate, dict) and gate.get("externalGate") == G1C_GATE_FAMILY for gate in gates
    ):
        errors.append(f"gates: missing external family {G1C_GATE_FAMILY}")

    covered_metrics: set[str] = set()
    for gate in g1c_gates:
        gate_id = str(gate.get("id", "<missing>"))
        row = g1c_row_for_gate(gate_id)
        if row is None:
            errors.append(f"gate {gate_id}: unexpected G1C-SEC row")
            continue
        if gate.get("status") != "approved":
            errors.append(f"gate {gate_id}: status must be approved")
        if gate.get("externalGate") != G1C_GATE_FAMILY:
            errors.append(f"gate {gate_id}: externalGate must be {G1C_GATE_FAMILY}")
        if gate.get("environmentId") != PHASE1C_ENVIRONMENT_ID:
            errors.append(f"gate {gate_id}: environmentId must be {PHASE1C_ENVIRONMENT_ID}")
        if gate.get("failureDisposition") != PHASE1C_FAILURE_DISPOSITION:
            errors.append(f"gate {gate_id}: failureDisposition must be {PHASE1C_FAILURE_DISPOSITION}")
        if gate.get("command") != G1C_COMMAND:
            errors.append(f"gate {gate_id}: command must be {G1C_COMMAND!r}")
        if gate.get("workload") != PHASE1C_WORKLOAD_REF:
            errors.append(f"gate {gate_id}: workload must be {PHASE1C_WORKLOAD_REF!r}")
        for field in ("owner", "approver"):
            if not isinstance(gate.get(field), str) or not gate[field].strip():
                errors.append(f"gate {gate_id}: {field} must be non-empty")
        if gate.get("owner") != row["owner"]:
            errors.append(f"gate {gate_id}: owner must be {row['owner']!r}")
        if gate.get("approver") != row["approver"]:
            errors.append(f"gate {gate_id}: approver must be {row['approver']!r}")
        evidence = gate.get("evidence")
        if not isinstance(evidence, str) or not evidence.strip():
            errors.append(f"gate {gate_id}: approved G1C gate requires evidence path")
        elif evidence != row["evidence"]:
            errors.append(f"gate {gate_id}: evidence must be {row['evidence']!r}")

        metric = (gate.get("metric") or {}).get("name")
        row_metrics = g1c_gate_metrics(row)
        primary = row_metrics[0]
        if metric != primary:
            errors.append(f"gate {gate_id}: metric.name must be {primary}")
        unit, statistic = g1c_metric_spec(primary)
        gate_metric = gate.get("metric") or {}
        if gate_metric.get("unit") != unit:
            errors.append(f"gate {gate_id}: metric.unit must be {unit}")
        if gate_metric.get("statistic") != statistic:
            errors.append(f"gate {gate_id}: metric.statistic must be {statistic}")
        threshold = gate.get("threshold") or {}
        expected_op, expected_val = PHASE1C_METRIC_THRESHOLDS[primary]
        if threshold.get("operator") != expected_op or threshold.get("value") != expected_val:
            errors.append(
                f"gate {gate_id}: threshold diverges from POC qualification for {primary}"
            )

        contracts, contract_errors = g1c_gate_metric_contracts(gate, row)
        errors.extend(contract_errors)
        contract_names: list[str] = []
        for contract in contracts:
            name = contract.get("name")
            if not isinstance(name, str):
                errors.append(f"gate {gate_id}: metricContracts item missing name")
                continue
            if name in contract_names:
                errors.append(f"gate {gate_id}: duplicate metricContracts entry {name}")
            contract_names.append(name)
            covered_metrics.add(name)
            if name not in PHASE1C_METRIC_THRESHOLDS:
                errors.append(f"gate {gate_id}: unknown metric contract {name}")
                continue
            expected_unit, expected_stat = g1c_metric_spec(name)
            if contract.get("unit") != expected_unit:
                errors.append(f"gate {gate_id}: metricContracts.{name}.unit must be {expected_unit}")
            if contract.get("statistic") != expected_stat:
                errors.append(
                    f"gate {gate_id}: metricContracts.{name}.statistic must be {expected_stat}"
                )
            contract_threshold = contract.get("threshold") or {}
            exp_op, exp_val = PHASE1C_METRIC_THRESHOLDS[name]
            if (
                contract_threshold.get("operator") != exp_op
                or contract_threshold.get("value") != exp_val
            ):
                errors.append(
                    f"gate {gate_id}: metricContracts.{name} threshold diverges from POC qualification"
                )
            expected_owner, expected_approver = g1c_metric_owners(name)
            if contract.get("owner") != expected_owner:
                errors.append(
                    f"gate {gate_id}: metricContracts.{name}.owner must be {expected_owner!r}"
                )
            if contract.get("approver") != expected_approver:
                errors.append(
                    f"gate {gate_id}: metricContracts.{name}.approver must be {expected_approver!r}"
                )

        missing_contracts = sorted(set(row_metrics) - set(contract_names))
        if missing_contracts:
            errors.append(f"gate {gate_id}: metricContracts missing metrics {missing_contracts}")
        extra_contracts = sorted(set(contract_names) - set(row_metrics))
        if extra_contracts:
            errors.append(f"gate {gate_id}: metricContracts unexpected metrics {extra_contracts}")

    missing_metrics = sorted(set(PHASE1C_METRIC_THRESHOLDS) - covered_metrics)
    if missing_metrics:
        errors.append(f"gates: G1C-SEC registry missing metrics {missing_metrics}")

    return errors


def phase1c_gate_report_errors(
    report: dict,
    *,
    registry: dict | None = None,
    root: Path | None = None,
    repo_root: Path | None = None,
    workspace_root: Path | None = None,
    template_mode: bool = False,
    git_commit_verifier: GitCommitVerifier | None = None,
) -> list[str]:
    """Fail closed on Phase 1C qualifying report invariants beyond JSON Schema."""
    workspace = workspace_root or repo_root or ROOT
    evidence_root = repo_root or ROOT
    verify_commit = git_commit_verifier or git_commit_resolves
    errors: list[str] = []
    if not isinstance(report, dict):
        return ["phase1c-report: report must be an object"]
    try:
        schema = load_json_yaml(PHASE1C_GATE_REPORT_SCHEMA)
    except (OSError, ValueError) as error:
        return [f"phase1c report schema: {error}"]
    errors.extend(schema_errors(strip_phase1c_harness_extensions(report), schema, "phase1c-report"))

    status = report.get("status")
    target_match = report.get("targetMatch")
    if status == "pass" and target_match is False:
        errors.append("phase1c-report: status pass requires targetMatch=true")
    if status == "pass" and template_mode:
        errors.append("phase1c-report: template/report must not claim status pass")
    coverage = report.get("coverageLimited")
    if status == "pass":
        if coverage:
            errors.append("phase1c-report: status pass forbidden when coverageLimited is set")
        elif not isinstance(coverage, list):
            errors.append("phase1c-report: status pass requires coverageLimited array")
    elif coverage:
        errors.append("phase1c-report: status pass forbidden when coverageLimited is set")
    if status == "not_run" and target_match is True:
        errors.append("phase1c-report: status not_run requires targetMatch=false")

    if not is_valid_iso8601_z(report.get("generatedAt")):
        errors.append("phase1c-report: generatedAt must be ISO8601 date-time")

    git = report.get("git")
    if not isinstance(git, dict):
        errors.append("phase1c-report: git must be an object")
    else:
        commit = git.get("commit")
        if not is_valid_git_sha(commit):
            errors.append("phase1c-report: git.commit must be a non-zero 40-char sha")
        elif status == "pass" and isinstance(commit, str):
            resolves, git_error = verify_commit(evidence_root, commit)
            if not resolves and git_error:
                errors.append(f"phase1c-report: {git_error}")
        if not isinstance(git.get("dirty"), bool):
            errors.append("phase1c-report: git.dirty must be boolean")
        elif status == "pass" and git.get("dirty") is not False:
            errors.append("phase1c-report: git.dirty must be false for status pass")

    if report.get("command") != G1C_COMMAND:
        errors.append(f"phase1c-report: command must be {G1C_COMMAND!r}")
    if report.get("environmentId") != PHASE1C_ENVIRONMENT_ID:
        errors.append(f"phase1c-report: environmentId must be {PHASE1C_ENVIRONMENT_ID!r}")
    if report.get("workloadProfileId") != PHASE1C_WORKLOAD_PROFILE_ID:
        errors.append(f"phase1c-report: workloadProfileId must be {PHASE1C_WORKLOAD_PROFILE_ID!r}")

    if not is_valid_sha256(report.get("denialManifestSha256")):
        errors.append("phase1c-report: denialManifestSha256 must be a non-zero sha256")

    binding = report.get("canonicalBinding")
    if not isinstance(binding, dict):
        errors.append("phase1c-report: canonicalBinding must be an object")
    else:
        for field in (
            "environmentSha256",
            "workloadSha256",
            "gatesSha256",
            "slaSha256",
            "thresholdDecisionsSha256",
        ):
            if not is_valid_sha256(binding.get(field)):
                errors.append(f"phase1c-report: canonicalBinding.{field} must be a non-zero sha256")
        if binding.get("registryRevision") != 1:
            errors.append("phase1c-report: canonicalBinding.registryRevision must be 1")
        if root is not None:
            live, live_errors = phase1c_canonical_fingerprints(root, workspace_root=workspace)
            errors.extend(live_errors)
            for field, live_value in live.items():
                if binding.get(field) != live_value:
                    errors.append(
                        f"phase1c-report: canonicalBinding.{field} diverges from canonical file"
                    )

    metrics = report.get("metrics")
    if not isinstance(metrics, dict):
        errors.append("phase1c-report: metrics must be an object")
    else:
        for metric in PHASE1C_METRIC_THRESHOLDS:
            if metric not in metrics:
                errors.append(f"phase1c-report: metrics missing {metric}")

    worker_proof = report.get("workerProof")
    if not isinstance(worker_proof, dict):
        errors.append("phase1c-report: missing workerProof")
    else:
        if not is_valid_iso8601_z(worker_proof.get("verifiedAt")):
            errors.append("phase1c-report: workerProof.verifiedAt must be ISO8601 date-time")
        if status == "pass":
            if worker_proof.get("runtimeRole") != "markhand_worker":
                errors.append("phase1c-report: workerProof.runtimeRole must be markhand_worker")
            if worker_proof.get("dedicatedDatabaseUrlVerified") is not True:
                errors.append(
                    "phase1c-report: workerProof.dedicatedDatabaseUrlVerified must be true"
                )
            if worker_proof.get("superuser") is not False:
                errors.append("phase1c-report: workerProof.superuser must be false")
            if worker_proof.get("bypassRls") is not False:
                errors.append("phase1c-report: workerProof.bypassRls must be false")

    decisions = report.get("thresholdDecisions")
    canonical = canonical_threshold_decisions()
    if not isinstance(decisions, list) or not decisions:
        errors.append("phase1c-report: thresholdDecisions must be non-empty")
    else:
        seen: set[str] = set()
        for index, decision in enumerate(decisions):
            path = f"phase1c-report.thresholdDecisions[{index}]"
            if not isinstance(decision, dict):
                errors.append(f"{path}: decision must be an object")
                continue
            metric = decision.get("metric")
            if not isinstance(metric, str):
                errors.append(f"{path}: metric must be a string")
                continue
            if metric in seen:
                errors.append(f"{path}: duplicate metric {metric}")
            seen.add(metric)
            if metric not in PHASE1C_METRIC_THRESHOLDS:
                errors.append(f"{path}: unknown metric {metric}")
                continue
            expected = next(item for item in canonical if item["metric"] == metric)
            for field in (
                "operator",
                "value",
                "unit",
                "statistic",
                "source",
                "provenance",
                "provenanceKind",
                "owner",
                "approver",
            ):
                if decision.get(field) != expected.get(field):
                    errors.append(f"{path}: {field} diverges from canonical threshold decision")
            if not is_valid_iso8601_z(decision.get("recordedAt")):
                errors.append(f"{path}: recordedAt must be ISO8601 date-time")
        missing = sorted(set(PHASE1C_METRIC_THRESHOLDS) - seen)
        if missing:
            errors.append(f"phase1c-report: thresholdDecisions missing metrics {missing}")

    redaction = report.get("redactionScan")
    if not isinstance(redaction, dict):
        errors.append("phase1c-report: redactionScan must be an object")
    elif status == "pass" and redaction.get("passed") is not True:
        errors.append("phase1c-report: redactionScan.passed must be true for status pass")

    vuln = report.get("vulnerabilityScan")
    if not isinstance(vuln, dict):
        errors.append("phase1c-report: vulnerabilityScan must be an object")
    else:
        undispositioned = vuln.get("undispositionedHighCritical")
        if not isinstance(undispositioned, int) or undispositioned < 0:
            errors.append(
                "phase1c-report: vulnerabilityScan.undispositionedHighCritical must be >= 0"
            )
        elif status == "pass" and undispositioned != 0:
            errors.append(
                "phase1c-report: vulnerabilityScan.undispositionedHighCritical must be 0 for status pass"
            )
        if status == "pass" and vuln.get("passed") is False:
            errors.append("phase1c-report: vulnerabilityScan.passed must not be false for status pass")

    registry_gates = {
        gate.get("id"): gate
        for gate in (registry or {}).get("gates", [])
        if isinstance(gate, dict) and str(gate.get("id", "")).startswith("G1C-SEC-")
    }
    gate_results = report.get("gateResults")
    if not isinstance(gate_results, list):
        errors.append("phase1c-report: gateResults must be an array")
    else:
        result_ids: list[str] = []
        for index, result in enumerate(gate_results):
            path = f"phase1c-report.gateResults[{index}]"
            if not isinstance(result, dict):
                errors.append(f"{path}: gate result must be an object")
                continue
            gate_id = result.get("gateId")
            if not isinstance(gate_id, str):
                errors.append(f"{path}: gateId must be a string")
                continue
            result_ids.append(gate_id)
            reg_gate = registry_gates.get(gate_id)
            if reg_gate is None:
                errors.append(f"{path}: unknown gateId {gate_id}")
                continue
            reg_metric = (reg_gate.get("metric") or {}).get("name")
            if result.get("externalGate") != G1C_GATE_FAMILY:
                errors.append(f"{path}: externalGate must be {G1C_GATE_FAMILY}")
            if result.get("failureDisposition") != PHASE1C_FAILURE_DISPOSITION:
                errors.append(f"{path}: failureDisposition must be {PHASE1C_FAILURE_DISPOSITION}")
            if result.get("metric") != reg_metric:
                errors.append(f"{path}: metric must match registry primary metric {reg_metric}")
            if result.get("evidence") != reg_gate.get("evidence"):
                errors.append(f"{path}: evidence must match registry evidence path")
            elif status == "pass" and not template_mode:
                evidence_rel = result.get("evidence")
                errors += phase1c_evidence_path_errors(
                    evidence_root,
                    evidence_rel,
                    context=path,
                )
                if isinstance(evidence_rel, str):
                    evidence_file = evidence_root / evidence_rel
                    if evidence_file.is_file():
                        try:
                            evidence_payload = json.loads(evidence_file.read_text(encoding="utf-8"))
                        except json.JSONDecodeError:
                            errors.append(f"{path}: evidence malformed at {evidence_rel}")
                        else:
                            errors += phase1c_evidence_qualification_errors(
                                evidence_payload,
                                context=f"{path}:{evidence_rel}",
                            )
                            row = g1c_row_for_gate(str(gate_id))
                            errors += phase1c_evidence_metric_binding_errors(
                                evidence_payload,
                                context=f"{path}:{evidence_rel}",
                                report_metrics=metrics if isinstance(metrics, dict) else None,
                                gate_row=row,
                            )
                            gate_id = result.get("gateId")
                            if evidence_payload.get("gateId") != gate_id:
                                errors.append(f"{path}: evidence gateId mismatch for {evidence_rel}")
                            if result.get("coverageLimited") is True:
                                errors.append(f"{path}: gateResult coverageLimited blocks pass")
                            if result.get("metricsObserved") is not True:
                                errors.append(f"{path}: gateResult metricsObserved must be true for pass")
            threshold = reg_gate.get("threshold") or {}
            value = result.get("value")
            if status == "pass":
                if result.get("pass") is not True:
                    errors.append(f"{path}: pass must be true for status pass")
                if isinstance(reg_metric, str) and reg_metric in PHASE1C_METRIC_THRESHOLDS:
                    op, limit = PHASE1C_METRIC_THRESHOLDS[reg_metric]
                    if not threshold_satisfied(value, op, limit):
                        errors.append(f"{path}: value {value!r} violates threshold for {reg_metric}")
            if isinstance(metrics, dict) and isinstance(reg_metric, str) and reg_metric in metrics:
                if value != metrics.get(reg_metric):
                    errors.append(f"{path}: value must match metrics.{reg_metric}")

        expected_ids = [str(row["id"]) for row in G1C_GATE_ROWS]
        if sorted(result_ids) != sorted(expected_ids):
            missing = sorted(set(expected_ids) - set(result_ids))
            extra = sorted(set(result_ids) - set(expected_ids))
            if missing:
                errors.append(f"phase1c-report: gateResults missing rows {missing}")
            if extra:
                errors.append(f"phase1c-report: gateResults unexpected rows {extra}")
            if len(result_ids) != len(set(result_ids)):
                errors.append("phase1c-report: gateResults contains duplicate gateId")

    if status == "pass" and isinstance(metrics, dict):
        for metric, (operator, limit) in PHASE1C_METRIC_THRESHOLDS.items():
            if metric in metrics and not threshold_satisfied(metrics[metric], operator, limit):
                errors.append(f"phase1c-report: metrics.{metric} violates canonical threshold")

    return errors


def phase1c_report_dir_errors(root: Path, registry: dict) -> list[str]:
    """Validate committed Phase 1C report contract when present."""
    report_path = root / "reports/phase-1c-gate/phase-1c-gate.json"
    template_path = root / "reports/phase-1c-gate/phase-1c-gate.template.json"
    if report_path.is_file():
        chosen = report_path
        template_mode = False
    elif template_path.is_file():
        chosen = template_path
        template_mode = True
    else:
        return [f"{template_path}: missing Phase 1C report template contract"]
    try:
        report = load_json_yaml(chosen)
    except (OSError, ValueError) as error:
        return [f"{chosen}: cannot read Phase 1C report: {error}"]
    if not isinstance(report, dict):
        return [f"{chosen}: Phase 1C report must be an object"]
    return phase1c_gate_report_errors(
        report,
        registry=registry,
        root=root,
        template_mode=template_mode,
    )


class GateValidatorTests(unittest.TestCase):
    def prepare_root(self, root: Path) -> None:
        (root / "environments").mkdir()
        (root / "workloads").mkdir(parents=True, exist_ok=True)
        (root / "schema").mkdir()
        for name in (
            "workload-profile.schema.json",
            "gates.schema.json",
            "environment.schema.json",
        ):
            (root / "schema" / name).write_text(
                (DEFAULT_ROOT / "schema" / name).read_text()
            )
        for path in (DEFAULT_ROOT / "environments").glob("*.yaml"):
            (root / "environments" / path.name).write_text(path.read_text())
        for path in (DEFAULT_ROOT / "workloads").glob("*.yaml"):
            (root / "workloads" / path.name).write_text(path.read_text())
        (root / "workload-profile.yaml").write_text(
            (DEFAULT_ROOT / "workload-profile.yaml").read_text()
        )

    def test_repository_registry_is_valid(self) -> None:
        self.assertEqual(validate(DEFAULT_ROOT), [])

    def test_denies_duplicate_missing_approver_unknown_environment_and_secret(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.prepare_root(root)
            workload = load_json_yaml(DEFAULT_ROOT / "workload-profile.yaml")
            gates = load_json_yaml(DEFAULT_ROOT / "gates.yaml")
            environment = load_json_yaml(DEFAULT_ROOT / "environments/on-prem-reference.yaml")
            gates["gates"][1]["id"] = gates["gates"][0]["id"]
            gates["gates"][0].pop("approver")
            gates["gates"][2]["environmentId"] = "missing"
            environment["notes"] = "postgres://user:password@host/db"
            (root / "workload-profile.yaml").write_text(json.dumps(workload))
            (root / "gates.yaml").write_text(json.dumps(gates))
            (root / "environments/on-prem-reference.yaml").write_text(json.dumps(environment))
            errors = validate(root)
            self.assertTrue(any("duplicate gate" in error for error in errors))
            self.assertTrue(any("missing approver" in error for error in errors))
            self.assertTrue(any("unknown environmentId" in error for error in errors))
            self.assertTrue(any("secret" in error for error in errors))

    def test_approved_gate_requires_numeric_threshold(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.prepare_root(root)
            workload = load_json_yaml(DEFAULT_ROOT / "workload-profile.yaml")
            gates = load_json_yaml(DEFAULT_ROOT / "gates.yaml")
            environment = load_json_yaml(DEFAULT_ROOT / "environments/on-prem-reference.yaml")
            gates["gates"][0]["status"] = "approved"
            gates["gates"][0]["threshold"]["value"] = None
            (root / "workload-profile.yaml").write_text(json.dumps(workload))
            (root / "gates.yaml").write_text(json.dumps(gates))
            (root / "environments/on-prem-reference.yaml").write_text(json.dumps(environment))
            self.assertTrue(
                any("threshold must be numeric" in error for error in validate(root))
            )

    def test_approved_profile_rejects_unresolved_or_non_positive_values(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.prepare_root(root)
            workload = load_json_yaml(DEFAULT_ROOT / "workload-profile.yaml")
            gates = load_json_yaml(DEFAULT_ROOT / "gates.yaml")
            environment = load_json_yaml(DEFAULT_ROOT / "environments/on-prem-reference.yaml")
            workload["openDecisions"][0]["status"] = "open"
            workload["openDecisions"][1].pop("resolution")
            workload["approver"] = ""
            workload["scale"]["orgCount"] = 0
            workload["loads"]["aggregate"]["tenantDistribution"] = ""
            workload["hardware"]["headroomPercent"]["cpu"] = 0
            environment["gpu"]["count"] = -1
            environment["approver"] = ""
            gates["gates"][0]["status"] = "proposed"
            gates["gates"][1]["metric"]["name"] = ""
            gates["gates"][2]["threshold"]["value"] = True
            (root / "workload-profile.yaml").write_text(json.dumps(workload))
            (root / "gates.yaml").write_text(json.dumps(gates))
            (root / "environments/on-prem-reference.yaml").write_text(json.dumps(environment))
            errors = validate(root)
            self.assertTrue(any("unresolved decisions" in error for error in errors))
            self.assertTrue(any("resolved decision requires resolution" in error for error in errors))
            self.assertTrue(any("approver must be non-empty" in error for error in errors))
            self.assertTrue(any("scale.orgCount must be positive" in error for error in errors))
            self.assertTrue(any("tenantDistribution must be non-empty" in error for error in errors))
            self.assertTrue(any("headroomPercent.cpu" in error for error in errors))
            self.assertTrue(any("gpu.count must be >= 0" in error for error in errors))
            self.assertTrue(any("metric.name must be non-empty" in error for error in errors))
            self.assertTrue(any("threshold must be numeric" in error for error in errors))
            self.assertTrue(any("non-approved gates" in error for error in errors))

    def test_environment_report_schema_rejects_empty_fingerprint_and_non_boolean_pass(self) -> None:
        schema = load_json_yaml(DEFAULT_ROOT / "reports/environment-report.schema.json")
        report = {
            "version": 1,
            "reportId": "report",
            "gateId": "gate",
            "generatedAt": "2026-07-18T00:00:00Z",
            "git": {"commit": "0" * 40, "dirty": False},
            "command": "command",
            "workloadProfileId": "profile",
            "environment": {"environmentId": "env", "fingerprint": {}},
            "fixtures": {"manifestSha256": "0" * 64},
            "result": {"metric": "metric", "value": 1, "pass": "yes"},
        }
        errors = schema_errors(report, schema, "report")
        self.assertTrue(any("fingerprint" in error and "required" in error for error in errors))
        self.assertTrue(any("report.result.pass" in error for error in errors))
        report["environment"]["fingerprint"] = {
            "gitCommit": "0" * 40,
            "workloadProfileId": "profile",
            "composeFileSha256": "0" * 64,
            "imageDigests": {"service": "sha256:synthetic"},
            "serviceVersions": {"service": "v1"},
            "fixtureManifestSha256": "0" * 64,
            "hardware": {
                "cpu": {"vendor": "", "model": "", "cores": 0, "threads": 0},
                "ramGb": 0,
                "disk": {"type": None, "capacityGb": 0, "iopsNote": None},
                "gpu": {"model": None, "vramGb": 0, "count": 0},
                "network": {"bandwidthGbps": 0, "latencyMsAssumed": -1},
                "os": {"distro": None, "arch": None},
            },
        }
        hardware_errors = schema_errors(report, schema, "report")
        self.assertTrue(any("hardware.cpu.cores" in error for error in hardware_errors))
        self.assertTrue(any("hardware.disk.type" in error for error in hardware_errors))
        self.assertTrue(any("hardware.gpu.model" in error for error in hardware_errors))
        self.assertTrue(
            any(
                "hardware.network" in error and "bandwidthMeasured" in error
                for error in hardware_errors
            )
        )
        self.assertTrue(any("hardware.os.arch" in error for error in hardware_errors))

    def test_approved_registry_rejects_unapproved_inputs_and_threshold_drift(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.prepare_root(root)
            workload = load_json_yaml(DEFAULT_ROOT / "workload-profile.yaml")
            gates = load_json_yaml(DEFAULT_ROOT / "gates.yaml")
            environment = load_json_yaml(DEFAULT_ROOT / "environments/on-prem-reference.yaml")
            workload["status"] = "proposed"
            environment["status"] = "proposed"
            next(
                gate for gate in gates["gates"] if gate["id"] == "G0-SLO-QUERY-P99"
            )["threshold"]["value"] = 999
            (root / "workload-profile.yaml").write_text(json.dumps(workload))
            (root / "gates.yaml").write_text(json.dumps(gates))
            (root / "environments/on-prem-reference.yaml").write_text(json.dumps(environment))
            errors = validate(root)
            self.assertTrue(any("requires approved workload" in error for error in errors))
            self.assertTrue(any("unapproved environments" in error for error in errors))
            self.assertTrue(any("diverges from approved workload" in error for error in errors))


class Phase1bGateReportConsistencyTests(unittest.TestCase):
    """Self-test for phase1b_gate_report_errors (P1B-O05 evidence drift)."""

    def _canonical(self) -> dict:
        return {
            "status": "pass",
            "versions": {"git": "f4f33cd1b"},
            "blockers": [],
        }

    def write_trio(self, gate_dir: Path, canonical: dict, summary: dict, md_status: str) -> None:
        gate_dir.mkdir(parents=True, exist_ok=True)
        (gate_dir / "o05-soak.json").write_text(json.dumps(canonical, indent=2, sort_keys=True) + "\n")
        (gate_dir / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
        (gate_dir / "phase-1b-gate.md").write_text(
            f"# Phase 1B soak / qualification\n\nStatus: **{md_status}**\n\nnotes\n"
        )

    def test_repository_reports_are_consistent(self) -> None:
        # Regression guard: the real committed evidence trio must agree.
        self.assertEqual(phase1b_gate_report_errors(PHASE1B_GATE_DIR), [])

    def test_matching_trio_has_no_errors(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            gate_dir = Path(temporary)
            canonical = self._canonical()
            self.write_trio(
                gate_dir,
                canonical,
                {
                    "status": "pass",
                    "versions": {"git": "f4f33cd1b"},
                    "blockers": [],
                },
                "pass",
            )
            self.assertEqual(phase1b_gate_report_errors(gate_dir), [])

    def test_stale_summary_status_and_git_and_blockers_detected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            gate_dir = Path(temporary)
            canonical = self._canonical()
            self.write_trio(
                gate_dir,
                canonical,
                {
                    "status": "incomplete",
                    "versions": {"git": "e3350d2"},
                    "blockers": ["prerequisites_incomplete"],
                },
                "pass",
            )
            errors = phase1b_gate_report_errors(gate_dir)
            self.assertTrue(any("status" in error and "summary.json" in error for error in errors))
            self.assertTrue(any("versions.git" in error for error in errors))
            self.assertTrue(any("blockers disagree" in error for error in errors))

    def test_stale_markdown_status_detected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            gate_dir = Path(temporary)
            canonical = self._canonical()
            self.write_trio(
                gate_dir,
                canonical,
                {
                    "status": "pass",
                    "versions": {"git": "f4f33cd1b"},
                    "blockers": [],
                },
                "incomplete",
            )
            errors = phase1b_gate_report_errors(gate_dir)
            self.assertTrue(
                any("phase-1b-gate.md" in error and "status line" in error for error in errors)
            )

    def test_missing_canonical_report_is_an_error(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            gate_dir = Path(temporary)
            gate_dir.mkdir(parents=True, exist_ok=True)
            errors = phase1b_gate_report_errors(gate_dir)
            self.assertTrue(any("cannot read canonical o05-soak.json" in error for error in errors))


def init_git_repo(repo_root: Path, *, marker: str = "fixture\n") -> str:
    """Initialize a disposable git repository and return its HEAD commit."""
    subprocess.run(["git", "init"], cwd=repo_root, check=True, capture_output=True)
    subprocess.run(
        ["git", "config", "user.email", "phase1c-test@example.com"],
        cwd=repo_root,
        check=True,
        capture_output=True,
    )
    subprocess.run(
        ["git", "config", "user.name", "phase1c-test"],
        cwd=repo_root,
        check=True,
        capture_output=True,
    )
    (repo_root / ".gitkeep").write_text(marker, encoding="utf-8")
    subprocess.run(["git", "add", ".gitkeep"], cwd=repo_root, check=True, capture_output=True)
    subprocess.run(
        ["git", "commit", "-m", "phase1c fixture"],
        cwd=repo_root,
        check=True,
        capture_output=True,
    )
    return subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo_root, text=True).strip()


def prepare_phase1c_fixture(temp_dir: Path) -> tuple[Path, Path]:
    """Copy Markhand Web + SLA doc into an isolated workspace for hermetic tests."""
    repo_root = temp_dir
    markhand_root = repo_root / "bench/markhand_web"
    shutil.copytree(DEFAULT_ROOT, markhand_root, dirs_exist_ok=True)
    sla_source = ROOT / PHASE1C_SLA_SOURCE
    sla_dest = repo_root / PHASE1C_SLA_SOURCE
    sla_dest.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(sla_source, sla_dest)
    if not (repo_root / ".git").exists():
        init_git_repo(repo_root)
    return markhand_root, repo_root


class Phase1cDescriptorSecurityTests(unittest.TestCase):
    """Fourth-review security tests: descriptor traversal and injected repo_root git binding."""

    def setUp(self) -> None:
        self._temp = tempfile.TemporaryDirectory()
        self.markhand_root, self.repo_root = prepare_phase1c_fixture(Path(self._temp.name))

    def tearDown(self) -> None:
        self._temp.cleanup()

    def _create_evidence_files(self) -> None:
        for evidence_path in PHASE1C_EVIDENCE_ALLOWLIST:
            path = self.repo_root / evidence_path
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('{"status":"pass","note":"test fixture"}\n', encoding="utf-8")

    def test_evidence_rejects_post_lstat_symlink_swap(self) -> None:
        """TOCTOU: directory traversal succeeds, then attacker swaps leaf before file open."""
        self._create_evidence_files()
        evidence = sorted(PHASE1C_EVIDENCE_ALLOWLIST)[0]
        evidence_file = self.repo_root / evidence
        decoy = self.repo_root / "bench/markhand_web/reports/phase-1c-gate/decoy-swap.json"
        decoy.write_text('{"status":"pass","note":"decoy after swap"}\n', encoding="utf-8")
        leaf_name = Path(evidence).name

        original_open = os.open
        swapped = False

        def open_then_swap(path: str, flags: int, mode: int = 0o777, *, dir_fd: int | None = None) -> int:
            nonlocal swapped
            open_kwargs: dict[str, object] = {}
            if dir_fd is not None and dir_fd != -1:
                open_kwargs["dir_fd"] = dir_fd
            if (
                not swapped
                and (flags & os.O_NOFOLLOW)
                and not (flags & os.O_DIRECTORY)
                and path == leaf_name
            ):
                swapped = True
                evidence_file.unlink()
                evidence_file.symlink_to(decoy)
            return original_open(path, flags, mode, **open_kwargs)

        with mock.patch("os.open", open_then_swap):
            errors = phase1c_evidence_path_errors(
                self.repo_root,
                evidence,
                context="test.evidence",
            )

        self.assertTrue(
            errors,
            "post-check symlink swap must be rejected by descriptor/no-follow validation",
        )
        self.assertTrue(
            any(
                keyword in error
                for error in errors
                for keyword in ("symlink", "O_NOFOLLOW", "descriptor", "no-follow")
            ),
            f"expected descriptor/no-follow rejection, got {errors}",
        )

    def test_report_commit_resolves_against_injected_repo_root_not_ambient_root(self) -> None:
        """Commit valid only in injected temp git repo must resolve via repo_root, not ambient ROOT."""
        with tempfile.TemporaryDirectory() as git_temp:
            git_repo = Path(git_temp)
            git_commit = init_git_repo(git_repo, marker="isolated-git-root\n")
            resolves_ambient, _ = git_commit_resolves(ROOT, git_commit)
            self.assertFalse(
                resolves_ambient,
                "sanity: temp-repo commit must not resolve against ambient workspace ROOT",
            )

            markhand_root, repo_root = prepare_phase1c_fixture(git_repo)
            for evidence_path in PHASE1C_EVIDENCE_ALLOWLIST:
                path = repo_root / evidence_path
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('{"status":"pass"}\n', encoding="utf-8")

            report = load_json_yaml(
                markhand_root / "reports/phase-1c-gate/phase-1c-gate.template.json"
            )
            report["status"] = "pass"
            report["targetMatch"] = True
            report["metrics"] = {
                metric: (1.0 if metric.endswith("_ratio") else 1 if metric == "worker_dedicated_role_verified" else 0)
                for metric in PHASE1C_METRIC_THRESHOLDS
            }
            report["workerProof"] = {
                "runtimeRole": "markhand_worker",
                "dedicatedDatabaseUrlVerified": True,
                "superuser": False,
                "bypassRls": False,
                "verifiedAt": "2026-08-04T00:00:00Z",
            }
            report["redactionScan"] = {"passed": True}
            report["vulnerabilityScan"]["passed"] = True
            report["vulnerabilityScan"]["undispositionedHighCritical"] = 0
            for result in report["gateResults"]:
                result["pass"] = True
                result["value"] = report["metrics"][result["metric"]]
            report["canonicalBinding"] = {
                "registryRevision": 1,
                **phase1c_canonical_fingerprints(markhand_root, workspace_root=repo_root)[0],
            }
            report["git"] = {"commit": git_commit, "dirty": False}

            errors = phase1c_gate_report_errors(
                report,
                registry=load_json_yaml(markhand_root / "gates.yaml"),
                root=markhand_root,
                repo_root=repo_root,
                workspace_root=repo_root,
            )

        git_errors = [error for error in errors if "git.commit" in error or "cat-file" in error]
        self.assertEqual(
            git_errors,
            [],
            f"injected repo_root commit must resolve without ambient ROOT; got {git_errors}",
        )


class Phase1cGateContractTests(unittest.TestCase):
    """Contract tests for G1C-SEC registry rows and phase-1c gate reports."""

    def setUp(self) -> None:
        self._temp = tempfile.TemporaryDirectory()
        self.markhand_root, self.repo_root = prepare_phase1c_fixture(Path(self._temp.name))

    def tearDown(self) -> None:
        self._temp.cleanup()

    def _load_template(self, markhand_root: Path | None = None) -> dict:
        root = markhand_root or self.markhand_root
        return load_json_yaml(root / "reports/phase-1c-gate/phase-1c-gate.template.json")

    def _load_registry(self, markhand_root: Path | None = None) -> dict:
        root = markhand_root or self.markhand_root
        return load_json_yaml(root / "gates.yaml")

    def _passing_report(
        self,
        *,
        markhand_root: Path | None = None,
        repo_root: Path | None = None,
    ) -> dict:
        markhand = markhand_root or self.markhand_root
        repo = repo_root or self.repo_root

        def metric_value(metric: str) -> int | float:
            if metric == "admin_mutation_audit_coverage_ratio":
                return 1.0
            if metric == "worker_dedicated_role_verified":
                return 1
            if metric in {"membership_acl_revoke_max_ms", "quiet_org_query_p95_ms"}:
                return 100
            return 0

        report = self._load_template(markhand)
        report["status"] = "pass"
        report["targetMatch"] = True
        report["metrics"] = {metric: metric_value(metric) for metric in PHASE1C_METRIC_THRESHOLDS}
        report["workerProof"] = {
            "runtimeRole": "markhand_worker",
            "dedicatedDatabaseUrlVerified": True,
            "superuser": False,
            "bypassRls": False,
            "verifiedAt": "2026-08-04T00:00:00Z",
        }
        report["redactionScan"] = {"passed": True}
        report["vulnerabilityScan"]["passed"] = True
        for result in report["gateResults"]:
            metric = result["metric"]
            report["metrics"][metric] = report["metrics"].get(metric, 0)
            result["value"] = report["metrics"][metric]
            result["pass"] = True
        report["canonicalBinding"] = {
            "registryRevision": 1,
            **phase1c_canonical_fingerprints(markhand, workspace_root=repo)[0],
        }
        report["git"] = {
            "commit": subprocess.check_output(
                ["git", "rev-parse", "HEAD"],
                cwd=repo,
                text=True,
            ).strip(),
            "dirty": False,
        }
        report["coverageLimited"] = []
        for result in report["gateResults"]:
            result["metricsObserved"] = True
            result["coverageLimited"] = False
        return report

    def _qualifying_metric_value(self, metric: str) -> int | float:
        if metric == "admin_mutation_audit_coverage_ratio":
            return 1.0
        if metric == "worker_dedicated_role_verified":
            return 1
        if metric in {"membership_acl_revoke_max_ms", "quiet_org_query_p95_ms"}:
            return 100
        return 0

    def _create_evidence_files(self, repo_root: Path | None = None) -> None:
        root = repo_root or self.repo_root
        for row in G1C_GATE_ROWS:
            path = root / str(row["evidence"])
            path.parent.mkdir(parents=True, exist_ok=True)
            metrics = {
                metric: self._qualifying_metric_value(metric)
                for metric in row["metrics"]  # type: ignore[index]
            }
            path.write_text(
                json.dumps(
                    {
                        "gateId": row["id"],
                        "status": "pass",
                        "metricsObserved": True,
                        "coverageLimited": False,
                        "metrics": metrics,
                    }
                )
                + "\n",
                encoding="utf-8",
            )

    def _remove_evidence_files(self, repo_root: Path | None = None) -> None:
        root = repo_root or self.repo_root
        for evidence_path in PHASE1C_EVIDENCE_ALLOWLIST:
            path = root / evidence_path
            if path.is_file() and not path.is_symlink():
                path.unlink()

    def test_repository_template_and_registry_validate(self) -> None:
        registry = self._load_registry()
        errors = phase1c_registry_contract_errors(
            registry,
            [load_json_yaml(self.markhand_root / "environments/phase1c-multi-org-poc.yaml")],
            root=self.markhand_root,
            workspace_root=self.repo_root,
        )
        self.assertEqual(errors, [])
        template_errors = phase1c_gate_report_errors(
            self._load_template(),
            registry=registry,
            root=self.markhand_root,
            repo_root=self.repo_root,
            workspace_root=self.repo_root,
            template_mode=True,
        )
        self.assertEqual(template_errors, [])

    def test_rejects_pass_when_evidence_coverage_limited_content(self) -> None:
        self._create_qualifying_evidence_files()
        path = self.repo_root / "bench/markhand_web/reports/phase-1c-gate/leakage.json"
        payload = json.loads(path.read_text(encoding="utf-8"))
        payload["coverageLimited"] = True
        path.write_text(json.dumps(payload) + "\n", encoding="utf-8")
        report = self._passing_report()
        errors = phase1c_gate_report_errors(
            report,
            registry=self._load_registry(),
            root=self.markhand_root,
            repo_root=self.repo_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("coverageLimited" in err for err in errors))

    def test_rejects_pass_when_evidence_lacks_metrics_observed(self) -> None:
        self._create_qualifying_evidence_files()
        path = self.repo_root / "bench/markhand_web/reports/phase-1c-gate/quota-recovery.json"
        payload = json.loads(path.read_text(encoding="utf-8"))
        payload.pop("metricsObserved", None)
        path.write_text(json.dumps(payload) + "\n", encoding="utf-8")
        report = self._passing_report()
        errors = phase1c_gate_report_errors(
            report,
            registry=self._load_registry(),
            root=self.markhand_root,
            repo_root=self.repo_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("metricsObserved" in err for err in errors))

    def test_rejects_pass_when_evidence_metrics_empty(self) -> None:
        self._create_qualifying_evidence_files()
        path = self.repo_root / "bench/markhand_web/reports/phase-1c-gate/leakage.json"
        payload = json.loads(path.read_text(encoding="utf-8"))
        payload["metrics"] = {}
        path.write_text(json.dumps(payload) + "\n", encoding="utf-8")
        report = self._passing_report()
        errors = phase1c_gate_report_errors(
            report,
            registry=self._load_registry(),
            root=self.markhand_root,
            repo_root=self.repo_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("metric" in err.lower() for err in errors))

    def test_rejects_pass_when_evidence_metric_mismatches_report(self) -> None:
        self._create_qualifying_evidence_files()
        path = self.repo_root / "bench/markhand_web/reports/phase-1c-gate/leakage.json"
        payload = json.loads(path.read_text(encoding="utf-8"))
        payload["metrics"]["cross_tenant_leakage_count"] = 99
        path.write_text(json.dumps(payload) + "\n", encoding="utf-8")
        report = self._passing_report()
        errors = phase1c_gate_report_errors(
            report,
            registry=self._load_registry(),
            root=self.markhand_root,
            repo_root=self.repo_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("match" in err.lower() or "metric" in err.lower() for err in errors))

    def test_rejects_pass_when_evidence_metric_violates_threshold(self) -> None:
        self._create_qualifying_evidence_files()
        path = self.repo_root / "bench/markhand_web/reports/phase-1c-gate/leakage.json"
        payload = json.loads(path.read_text(encoding="utf-8"))
        payload["metrics"]["cross_tenant_leakage_count"] = 5
        path.write_text(json.dumps(payload) + "\n", encoding="utf-8")
        report = self._passing_report()
        errors = phase1c_gate_report_errors(
            report,
            registry=self._load_registry(),
            root=self.markhand_root,
            repo_root=self.repo_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("threshold" in err.lower() or "violat" in err.lower() for err in errors))

    def test_rejects_pass_when_gate_result_missing_metrics_observed(self) -> None:
        self._create_qualifying_evidence_files()
        report = self._passing_report()
        report["gateResults"][0].pop("metricsObserved", None)
        errors = phase1c_gate_report_errors(
            report,
            registry=self._load_registry(),
            root=self.markhand_root,
            repo_root=self.repo_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("metricsObserved" in err for err in errors))

    def _create_qualifying_evidence_files(self) -> None:
        for row in G1C_GATE_ROWS:
            path = self.repo_root / str(row["evidence"])
            path.parent.mkdir(parents=True, exist_ok=True)
            metrics = {
                metric: self._qualifying_metric_value(metric)
                for metric in row["metrics"]  # type: ignore[index]
            }
            path.write_text(
                json.dumps(
                    {
                        "gateId": row["id"],
                        "status": "pass",
                        "metricsObserved": True,
                        "coverageLimited": False,
                        "metrics": metrics,
                    }
                )
                + "\n",
                encoding="utf-8",
            )

    def test_registry_requires_starvation_events_in_noisy_neighbor_contracts(self) -> None:
        registry = self._load_registry()
        noisy = next(g for g in registry["gates"] if g["id"] == "G1C-SEC-NOISY-NEIGHBOR")
        contracts = noisy.get("metricContracts") or []
        names = {item["name"] for item in contracts if isinstance(item, dict)}
        self.assertIn("starvation_events", names)
        errors = phase1c_registry_contract_errors(
            registry,
            [load_json_yaml(self.markhand_root / "environments/phase1c-multi-org-poc.yaml")],
            root=self.markhand_root,
            workspace_root=self.repo_root,
        )
        self.assertEqual(errors, [])
        noisy["metricContracts"] = [c for c in contracts if c.get("name") != "starvation_events"]
        errors = phase1c_registry_contract_errors(
            registry,
            [load_json_yaml(self.markhand_root / "environments/phase1c-multi-org-poc.yaml")],
            root=self.markhand_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("starvation_events" in error for error in errors))

    def test_registry_rejects_loads_peak_workload_binding(self) -> None:
        registry = self._load_registry()
        gate = next(g for g in registry["gates"] if g["id"] == "G1C-SEC-LEAKAGE")
        gate["workload"] = "loads.peak"
        errors = phase1c_registry_contract_errors(
            registry,
            [load_json_yaml(self.markhand_root / "environments/phase1c-multi-org-poc.yaml")],
            root=self.markhand_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("workload must be" in error for error in errors))

    def test_registry_rejects_unapproved_registry_status(self) -> None:
        registry = self._load_registry()
        registry["registryStatus"] = "proposed"
        errors = phase1c_registry_contract_errors(
            registry,
            [load_json_yaml(self.markhand_root / "environments/phase1c-multi-org-poc.yaml")],
            root=self.markhand_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("registryStatus approved" in error for error in errors))

    def test_registry_rejects_unknown_gate_id_without_crashing(self) -> None:
        registry = self._load_registry()
        registry["gates"].append(
            {
                "id": "G1C-SEC-UNKNOWN",
                "externalGate": G1C_GATE_FAMILY,
                "metric": {"name": "cross_tenant_leakage_count", "unit": "count", "statistic": "max"},
                "workload": PHASE1C_WORKLOAD_REF,
                "threshold": {"operator": "==", "value": 0},
                "command": G1C_COMMAND,
                "environmentId": PHASE1C_ENVIRONMENT_ID,
                "owner": "security-owner",
                "approver": "security-owner",
                "status": "approved",
                "failureDisposition": PHASE1C_FAILURE_DISPOSITION,
                "evidence": "bench/markhand_web/reports/phase-1c-gate/leakage.json",
            }
        )
        errors = phase1c_registry_contract_errors(
            registry,
            [load_json_yaml(self.markhand_root / "environments/phase1c-multi-org-poc.yaml")],
            root=self.markhand_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("requires canonical G1C id" in error for error in errors))

    def test_report_schema_rejects_unknown_keys_and_all_zero_hashes(self) -> None:
        schema = load_json_yaml(PHASE1C_GATE_REPORT_SCHEMA)
        report = self._passing_report()
        report["unexpected"] = True
        report["denialManifestSha256"] = ALL_ZERO_SHA256
        errors = schema_errors(report, schema, "phase1c-report")
        self.assertTrue(any("unknown property" in error for error in errors))

    def test_report_rejects_pass_with_target_match_false(self) -> None:
        report = self._passing_report()
        report["targetMatch"] = False
        errors = phase1c_gate_report_errors(
            report,
            registry=self._load_registry(),
            root=self.markhand_root,
            repo_root=self.repo_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("targetMatch" in error for error in errors))

    def test_report_rejects_metric_threshold_violation_on_pass(self) -> None:
        report = self._passing_report()
        report["metrics"]["cross_tenant_leakage_count"] = 1
        errors = phase1c_gate_report_errors(
            report,
            registry=self._load_registry(),
            root=self.markhand_root,
            repo_root=self.repo_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("violates canonical threshold" in error for error in errors))

    def test_report_rejects_duplicate_and_unknown_threshold_decisions(self) -> None:
        report = self._passing_report()
        decisions = list(canonical_threshold_decisions())
        decisions.append(dict(decisions[0]))
        decisions[1]["metric"] = "unknown_metric"
        report["thresholdDecisions"] = decisions
        errors = phase1c_gate_report_errors(
            report,
            registry=self._load_registry(),
            root=self.markhand_root,
            repo_root=self.repo_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("duplicate metric" in error for error in errors))
        self.assertTrue(any("unknown metric" in error for error in errors))

    def test_report_rejects_wrong_threshold_decision_owner_and_provenance(self) -> None:
        report = self._passing_report()
        report["thresholdDecisions"][0]["provenanceKind"] = "external-sign-off"
        errors = phase1c_gate_report_errors(
            report,
            registry=self._load_registry(),
            root=self.markhand_root,
            repo_root=self.repo_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("provenanceKind diverges" in error for error in errors))

    def test_report_rejects_missing_gate_result_and_false_child_pass(self) -> None:
        report = self._passing_report()
        report["gateResults"] = report["gateResults"][:-1]
        errors = phase1c_gate_report_errors(
            report,
            registry=self._load_registry(),
            root=self.markhand_root,
            repo_root=self.repo_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("gateResults missing rows" in error for error in errors))
        report = self._passing_report()
        report["gateResults"][0]["pass"] = False
        errors = phase1c_gate_report_errors(
            report,
            registry=self._load_registry(),
            root=self.markhand_root,
            repo_root=self.repo_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("pass must be true" in error for error in errors))

    def test_report_rejects_evidence_and_registry_mismatch(self) -> None:
        report = self._passing_report()
        report["gateResults"][0]["evidence"] = "wrong/path.json"
        errors = phase1c_gate_report_errors(
            report,
            registry=self._load_registry(),
            root=self.markhand_root,
            repo_root=self.repo_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("evidence must match registry" in error for error in errors))

    def test_report_rejects_failed_redaction_and_vulnerability_scan_on_pass(self) -> None:
        report = self._passing_report()
        report["redactionScan"]["passed"] = False
        report["vulnerabilityScan"]["passed"] = False
        report["vulnerabilityScan"]["undispositionedHighCritical"] = 2
        errors = phase1c_gate_report_errors(
            report,
            registry=self._load_registry(),
            root=self.markhand_root,
            repo_root=self.repo_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("redactionScan.passed" in error for error in errors))
        self.assertTrue(any("undispositionedHighCritical" in error for error in errors))

    def test_report_rejects_wrong_canonical_binding_hash(self) -> None:
        report = self._passing_report()
        report["canonicalBinding"]["gatesSha256"] = ALL_ZERO_SHA256
        errors = phase1c_gate_report_errors(
            report,
            registry=self._load_registry(),
            root=self.markhand_root,
            repo_root=self.repo_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("gatesSha256" in error for error in errors))

    def test_report_rejects_malformed_types_without_crashing(self) -> None:
        report = self._passing_report()
        report["gateResults"] = "not-an-array"
        report["thresholdDecisions"] = "bad"
        errors = phase1c_gate_report_errors(
            report,
            registry=self._load_registry(),
            root=self.markhand_root,
            repo_root=self.repo_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("gateResults must be an array" in error for error in errors))
        self.assertTrue(any("thresholdDecisions must be non-empty" in error for error in errors))

    def test_main_validation_includes_phase1c_template(self) -> None:
        self.assertEqual(
            run_markhand_gate_validation(self.markhand_root),
            [],
        )

    def test_schema_max_items_enforced(self) -> None:
        schema = load_json_yaml(PHASE1C_GATE_REPORT_SCHEMA)
        report = self._passing_report()
        report["thresholdDecisions"].append(dict(report["thresholdDecisions"][0]))
        errors = schema_errors(report, schema, "phase1c-report")
        self.assertTrue(any("too many items" in error for error in errors))

    def test_gates_schema_rejects_unknown_gate_property(self) -> None:
        schema = load_json_yaml(DEFAULT_ROOT / "schema/gates.schema.json")
        gate = {"id": "X", "unexpected": True}
        errors = schema_errors(gate, schema["properties"]["gates"]["items"], "gate")
        self.assertTrue(any("unknown property" in error for error in errors))

    def test_registry_rejects_absent_metric_contracts_on_multi_metric_gate(self) -> None:
        registry = self._load_registry()
        noisy = next(g for g in registry["gates"] if g["id"] == "G1C-SEC-NOISY-NEIGHBOR")
        noisy.pop("metricContracts", None)
        errors = phase1c_registry_contract_errors(
            registry,
            [load_json_yaml(self.markhand_root / "environments/phase1c-multi-org-poc.yaml")],
            root=self.markhand_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("metricContracts required" in error for error in errors))

    def test_registry_rejects_empty_metric_contracts(self) -> None:
        registry = self._load_registry()
        noisy = next(g for g in registry["gates"] if g["id"] == "G1C-SEC-NOISY-NEIGHBOR")
        noisy["metricContracts"] = []
        errors = phase1c_registry_contract_errors(
            registry,
            [load_json_yaml(self.markhand_root / "environments/phase1c-multi-org-poc.yaml")],
            root=self.markhand_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("metricContracts required" in error for error in errors))

    def test_registry_rejects_wrong_type_metric_contracts(self) -> None:
        registry = self._load_registry()
        noisy = next(g for g in registry["gates"] if g["id"] == "G1C-SEC-NOISY-NEIGHBOR")
        noisy["metricContracts"] = "bad"
        errors = phase1c_registry_contract_errors(
            registry,
            [load_json_yaml(self.markhand_root / "environments/phase1c-multi-org-poc.yaml")],
            root=self.markhand_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("metricContracts required" in error for error in errors))

    def test_registry_rejects_duplicate_metric_contracts(self) -> None:
        registry = self._load_registry()
        noisy = next(g for g in registry["gates"] if g["id"] == "G1C-SEC-NOISY-NEIGHBOR")
        contracts = list(noisy["metricContracts"])
        contracts.append(dict(contracts[0]))
        noisy["metricContracts"] = contracts
        errors = phase1c_registry_contract_errors(
            registry,
            [load_json_yaml(self.markhand_root / "environments/phase1c-multi-org-poc.yaml")],
            root=self.markhand_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("duplicate metricContracts" in error for error in errors))

    def test_registry_rejects_wrong_contract_owner_approver(self) -> None:
        registry = self._load_registry()
        noisy = next(g for g in registry["gates"] if g["id"] == "G1C-SEC-NOISY-NEIGHBOR")
        noisy["metricContracts"][0]["owner"] = "wrong-owner"
        errors = phase1c_registry_contract_errors(
            registry,
            [load_json_yaml(self.markhand_root / "environments/phase1c-multi-org-poc.yaml")],
            root=self.markhand_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("metricContracts.quiet_org_query_p95_ms.owner" in error for error in errors))

    def test_registry_rejects_workload_org_count_drift(self) -> None:
        registry = self._load_registry()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "workloads").mkdir(parents=True)
            (root / "environments").mkdir()
            profile = load_json_yaml(PHASE1C_WORKLOAD_FILE)
            profile["orgCount"] = 3
            (root / "workloads/phase1c-multi-org.yaml").write_text(json.dumps(profile))
            shutil.copytree(DEFAULT_ROOT / "environments", root / "environments", dirs_exist_ok=True)
            (root / "workload-profile.yaml").write_text(
                (DEFAULT_ROOT / "workload-profile.yaml").read_text()
            )
            errors = phase1c_registry_contract_errors(
                registry,
                [load_json_yaml(DEFAULT_ROOT / "environments/phase1c-multi-org-poc.yaml")],
                root=root,
            )
            self.assertTrue(any("orgCount must be 2" in error for error in errors))

    def test_registry_rejects_missing_canonical_environment_file(self) -> None:
        registry = self._load_registry()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "workloads").mkdir(parents=True)
            (root / "workloads/phase1c-multi-org.yaml").write_text(
                PHASE1C_WORKLOAD_FILE.read_text()
            )
            (root / "workload-profile.yaml").write_text(
                (DEFAULT_ROOT / "workload-profile.yaml").read_text()
            )
            errors = phase1c_registry_contract_errors(
                registry,
                [load_json_yaml(DEFAULT_ROOT / "environments/phase1c-multi-org-poc.yaml")],
                root=root,
            )
            self.assertTrue(any("missing canonical file" in error for error in errors))

    def test_report_pass_requires_evidence_files_exist(self) -> None:
        self._create_evidence_files()
        report = self._passing_report()
        errors = phase1c_gate_report_errors(
            report,
            registry=self._load_registry(),
            root=self.markhand_root,
            repo_root=self.repo_root,
            workspace_root=self.repo_root,
        )
        self.assertEqual(errors, [])
        self._remove_evidence_files()
        errors = phase1c_gate_report_errors(
            report,
            registry=self._load_registry(),
            root=self.markhand_root,
            repo_root=self.repo_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(
            any("evidence file missing" in error or "evidence path component missing" in error for error in errors)
        )

    def test_report_pass_rejects_unsafe_evidence_paths(self) -> None:
        report = self._passing_report()
        report["gateResults"][0]["evidence"] = "/etc/passwd"
        errors = phase1c_gate_report_errors(
            report,
            registry=self._load_registry(),
            root=self.markhand_root,
            repo_root=self.repo_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("evidence must match registry" in error for error in errors))

    def test_report_pass_rejects_dirty_git(self) -> None:
        self._create_evidence_files()
        report = self._passing_report()
        report["git"]["dirty"] = True
        errors = phase1c_gate_report_errors(
            report,
            registry=self._load_registry(),
            root=self.markhand_root,
            repo_root=self.repo_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("git.dirty must be false" in error for error in errors))

    def test_report_pass_rejects_nonexistent_git_commit(self) -> None:
        self._create_evidence_files()
        report = self._passing_report()
        report["git"]["commit"] = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
        errors = phase1c_gate_report_errors(
            report,
            registry=self._load_registry(),
            root=self.markhand_root,
            repo_root=self.repo_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("does not resolve to a commit object" in error for error in errors))

    def test_report_pass_rejects_zero_git_commit(self) -> None:
        report = self._passing_report()
        report["git"]["commit"] = ALL_ZERO_GIT_SHA
        errors = phase1c_gate_report_errors(
            report,
            registry=self._load_registry(),
            root=self.markhand_root,
            repo_root=self.repo_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("git.commit must be a non-zero" in error for error in errors))

    def test_report_rejects_missing_canonical_sla_file_binding(self) -> None:
        report = self._passing_report()
        real_sha256 = canonical_file_sha256

        def fake_sha256(path: Path) -> tuple[str | None, str | None]:
            if str(path).endswith("markhand-web-sla-targets.md"):
                return None, f"missing canonical file {path}"
            return real_sha256(path)

        with mock.patch(__name__ + ".canonical_file_sha256", side_effect=fake_sha256):
            errors = phase1c_gate_report_errors(
                report,
                registry=self._load_registry(),
                root=self.markhand_root,
                repo_root=self.repo_root,
                workspace_root=self.repo_root,
            )
        self.assertTrue(any("missing canonical file" in error for error in errors))

    def test_report_pass_rejects_leaf_symlink_evidence(self) -> None:
        self._create_evidence_files()
        evidence = sorted(PHASE1C_EVIDENCE_ALLOWLIST)[0]
        target = self.repo_root / "real-evidence-leaf.json"
        target.write_text('{"status":"pass","note":"real target"}\n', encoding="utf-8")
        link = self.repo_root / evidence
        link.unlink()
        link.symlink_to(target)
        report = self._passing_report()
        errors = phase1c_gate_report_errors(
            report,
            registry=self._load_registry(),
            root=self.markhand_root,
            repo_root=self.repo_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(
            any(
                "symlink" in error
                or "O_NOFOLLOW" in error
                or "descriptor" in error
                or "not a directory" in error
                for error in errors
            )
        )

    def test_report_pass_rejects_parent_symlink_in_evidence_path(self) -> None:
        report = self._passing_report()
        evidence = sorted(PHASE1C_EVIDENCE_ALLOWLIST)[0]
        real_dir = self.repo_root / "bench/markhand_web/reports/phase-1c-real-gate"
        real_dir.mkdir(parents=True, exist_ok=True)
        real_file = real_dir / Path(evidence).name
        real_file.write_text('{"status":"pass","note":"real target"}\n', encoding="utf-8")
        gate_dir = self.repo_root / "bench/markhand_web/reports/phase-1c-gate"
        if gate_dir.exists() or gate_dir.is_symlink():
            if gate_dir.is_symlink():
                gate_dir.unlink()
            else:
                shutil.rmtree(gate_dir)
        gate_dir.symlink_to(real_dir)
        errors = phase1c_gate_report_errors(
            report,
            registry=self._load_registry(),
            root=self.markhand_root,
            repo_root=self.repo_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(
            any(
                "symlink" in error
                or "O_NOFOLLOW" in error
                or "descriptor" in error
                or "not a directory" in error
                for error in errors
            )
        )

    def test_registry_rejects_g1c_family_on_noncanonical_id(self) -> None:
        registry = self._load_registry()
        registry["gates"].append(
            {
                "id": "G1C-SEC-ROGUE",
                "externalGate": G1C_GATE_FAMILY,
                "metric": {"name": "cross_tenant_leakage_count", "unit": "count", "statistic": "max"},
                "workload": PHASE1C_WORKLOAD_REF,
                "threshold": {"operator": "==", "value": 0},
                "command": G1C_COMMAND,
                "environmentId": PHASE1C_ENVIRONMENT_ID,
                "owner": "security-owner",
                "approver": "security-owner",
                "status": "approved",
                "failureDisposition": PHASE1C_FAILURE_DISPOSITION,
                "evidence": "bench/markhand_web/reports/phase-1c-gate/leakage.json",
            }
        )
        errors = phase1c_registry_contract_errors(
            registry,
            [load_json_yaml(self.markhand_root / "environments/phase1c-multi-org-poc.yaml")],
            root=self.markhand_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("requires canonical G1C id" in error for error in errors))

    def test_registry_rejects_canonical_g1c_id_with_wrong_family(self) -> None:
        registry = self._load_registry()
        gate = next(g for g in registry["gates"] if g["id"] == "G1C-SEC-LEAKAGE")
        gate["externalGate"] = "G0-SEC"
        errors = phase1c_registry_contract_errors(
            registry,
            [load_json_yaml(self.markhand_root / "environments/phase1c-multi-org-poc.yaml")],
            root=self.markhand_root,
            workspace_root=self.repo_root,
        )
        self.assertTrue(any("requires externalGate G1C-SEC" in error for error in errors))

    def test_report_schema_rejects_audit_ratio_above_maximum(self) -> None:
        schema = load_json_yaml(PHASE1C_GATE_REPORT_SCHEMA)
        report = self._passing_report()
        report["metrics"]["admin_mutation_audit_coverage_ratio"] = 1.1
        errors = schema_errors(report, schema, "phase1c-report")
        self.assertTrue(any("above schema maximum" in error for error in errors))

    def test_report_schema_rejects_audit_ratio_below_minimum(self) -> None:
        schema = load_json_yaml(PHASE1C_GATE_REPORT_SCHEMA)
        report = self._load_template()
        report["metrics"]["admin_mutation_audit_coverage_ratio"] = -0.1
        errors = schema_errors(report, schema, "phase1c-report")
        self.assertTrue(any("below schema minimum" in error for error in errors))

    def test_report_schema_rejects_boolean_metric_as_number(self) -> None:
        schema = load_json_yaml(PHASE1C_GATE_REPORT_SCHEMA)
        report = self._load_template()
        report["metrics"]["starvation_events"] = True
        errors = schema_errors(report, schema, "phase1c-report")
        self.assertTrue(
            any("boolean is not a schema number" in error or "schema type must be integer" in error for error in errors)
        )

    def test_main_entrypoint_includes_phase1c_report_validation(self) -> None:
        template_path = self.markhand_root / "reports/phase-1c-gate/phase-1c-gate.template.json"
        original = template_path.read_bytes()
        try:
            broken = load_json_yaml(template_path)
            broken["status"] = "pass"
            broken["targetMatch"] = True
            template_path.write_text(json.dumps(broken), encoding="utf-8")
            self.assertEqual(validate(self.markhand_root), [])
            full_errors = run_markhand_gate_validation(self.markhand_root)
            self.assertTrue(
                any("template/report must not claim status pass" in error for error in full_errors)
            )
            with mock.patch(__name__ + ".phase1c_report_dir_errors", return_value=[]):
                bypassed = run_markhand_gate_validation(self.markhand_root)
            self.assertFalse(
                any("template/report must not claim status pass" in error for error in bypassed)
            )
        finally:
            template_path.write_bytes(original)


class Phase1cHermeticRegressionTests(unittest.TestCase):
    def test_self_tests_leave_tracked_template_and_evidence_unmodified(self) -> None:
        template_path = PHASE1C_REPORT_TEMPLATE
        template_before = template_path.read_bytes()
        sentinel = ROOT / sorted(PHASE1C_EVIDENCE_ALLOWLIST)[0]
        sentinel_existed = sentinel.exists()
        sentinel_before = sentinel.read_bytes() if sentinel_existed else b""

        loader = unittest.defaultTestLoader
        suite = loader.loadTestsFromTestCase(Phase1cGateContractTests)
        result = unittest.TextTestRunner(verbosity=0).run(suite)
        self.assertTrue(result.wasSuccessful())

        self.assertEqual(template_path.read_bytes(), template_before)
        if sentinel_existed:
            self.assertEqual(sentinel.read_bytes(), sentinel_before)
        else:
            self.assertFalse(sentinel.exists())


def run_markhand_gate_validation(root: Path) -> list[str]:
    """Full gate validation orchestration used by the CLI entrypoint."""
    errors = validate(root)
    errors += phase1b_gate_report_errors(root / "reports/phase-1b-gate")
    try:
        registry = load_json_yaml(root / "gates.yaml")
    except (OSError, ValueError) as error:
        errors.append(f"gates.yaml: cannot read registry: {error}")
        return errors
    errors += phase1c_report_dir_errors(root, registry)
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=DEFAULT_ROOT)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        loader = unittest.defaultTestLoader
        suite = unittest.TestSuite()
        suite.addTests(loader.loadTestsFromTestCase(GateValidatorTests))
        suite.addTests(loader.loadTestsFromTestCase(Phase1bGateReportConsistencyTests))
        suite.addTests(loader.loadTestsFromTestCase(Phase1cDescriptorSecurityTests))
        suite.addTests(loader.loadTestsFromTestCase(Phase1cGateContractTests))
        suite.addTests(loader.loadTestsFromTestCase(Phase1cHermeticRegressionTests))
        return 0 if unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful() else 1
    try:
        errors = run_markhand_gate_validation(args.root)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"gate registry error: {error}", file=sys.stderr)
        return 1
    if errors:
        print("gate registry validation failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    print("Markhand workload and gate registry valid")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
