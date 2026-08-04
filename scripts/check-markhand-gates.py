#!/usr/bin/env python3
"""Validate Markhand Web workload, hardware and decision-gate registry."""

from __future__ import annotations

import argparse
import json
import math
import re
import sys
import tempfile
import unittest
from pathlib import Path


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
GATE_FAMILIES = {"G0-ARCH", "G0-RET", "G0-SEC", "G0-CAP", "G0-SLO", "G0-LIC"}
PHASE1C_GATE_REPORT_SCHEMA = DEFAULT_ROOT / "schema/phase1c-gate-report.schema.json"
PHASE1C_ENVIRONMENT_ID = "phase1c-multi-org-poc"
PHASE1C_WORKLOAD_PROFILE_ID = "phase1c-multi-org"
G1C_GATE_FAMILY = "G1C-SEC"
PHASE1C_FAILURE_DISPOSITION = "block-phase-1c"
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
G1C_GATE_ROWS: tuple[dict[str, str | None], ...] = (
    {
        "id": "G1C-SEC-LEAKAGE",
        "metric": "cross_tenant_leakage_count",
        "owner": "security-owner",
        "approver": "security-owner",
        "evidence": "bench/markhand_web/reports/phase-1c-gate/leakage.json",
    },
    {
        "id": "G1C-SEC-REVOKE",
        "metric": "membership_acl_revoke_max_ms",
        "owner": "security-owner",
        "approver": "operations-owner",
        "evidence": "bench/markhand_web/reports/phase-1c-gate/revoke.json",
    },
    {
        "id": "G1C-SEC-ACL-CACHE",
        "metric": "post_commit_stale_authorizations",
        "owner": "security-owner",
        "approver": "security-owner",
        "evidence": "bench/markhand_web/reports/phase-1c-gate/acl-cache.json",
    },
    {
        "id": "G1C-SEC-QUOTA-RECOVERY",
        "metric": "quota_drift_after_recovery",
        "owner": "operations-owner",
        "approver": "operations-owner",
        "evidence": "bench/markhand_web/reports/phase-1c-gate/quota-recovery.json",
    },
    {
        "id": "G1C-SEC-NOISY-NEIGHBOR",
        "metric": "quiet_org_query_p95_ms",
        "owner": "operations-owner",
        "approver": "operations-owner",
        "evidence": "bench/markhand_web/reports/phase-1c-gate/noisy-neighbor.json",
    },
    {
        "id": "G1C-SEC-AUDIT-COVERAGE",
        "metric": "admin_mutation_audit_coverage_ratio",
        "owner": "security-owner",
        "approver": "security-owner",
        "evidence": "bench/markhand_web/reports/phase-1c-gate/audit-coverage.json",
    },
    {
        "id": "G1C-SEC-WORKER-ROLE",
        "metric": "worker_dedicated_role_verified",
        "owner": "security-owner",
        "approver": "operations-owner",
        "evidence": "bench/markhand_web/reports/phase-1c-gate/worker-role.json",
    },
    {
        "id": "G1C-SEC-CONTAINER-VULNS",
        "metric": "undispositioned_high_critical_count",
        "owner": "security-owner",
        "approver": "security-owner",
        "evidence": "bench/markhand_web/reports/phase-1c-gate/container-vulns.json",
    },
    {
        "id": "G1C-SEC-STALE-TOKENS",
        "metric": "post_commit_stale_authorizations",
        "owner": "security-owner",
        "approver": "security-owner",
        "evidence": "bench/markhand_web/reports/phase-1c-gate/stale-tokens.json",
    },
    {
        "id": "G1C-SEC-QDRANT-FAIL-CLOSED",
        "metric": "cross_tenant_leakage_count",
        "owner": "security-owner",
        "approver": "security-owner",
        "evidence": "bench/markhand_web/reports/phase-1c-gate/qdrant-fail-closed.json",
    },
)
OPERATORS = {">=", ">", "<=", "<", "=="}
FAILURE_DISPOSITIONS = {
    "block-phase-1b",
    "block-phase-1c",
    "block-phase-4",
    "block-issue",
    "research-only",
    "waive-with-adr",
}
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
    if numeric(value):
        if "minimum" in schema and value < schema["minimum"]:
            errors.append(f"{path}: number is below schema minimum")
        if "exclusiveMinimum" in schema and value <= schema["exclusiveMinimum"]:
            errors.append(f"{path}: number is below schema exclusiveMinimum")
    if isinstance(value, dict):
        if len(value) < schema.get("minProperties", 0):
            errors.append(f"{path}: object has too few properties")
        for field in schema.get("required", []):
            if field not in value:
                errors.append(f"{path}: schema missing required field {field}")
        for field, child_schema in schema.get("properties", {}).items():
            if field in value:
                errors.extend(schema_errors(value[field], child_schema, f"{path}.{field}"))
    if isinstance(value, list):
        if len(value) < schema.get("minItems", 0):
            errors.append(f"{path}: array has too few items")
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
    require_g1c_rows: bool = True,
) -> list[str]:
    """Validate Phase 1C G1C-SEC registry rows and qualifying environment binding."""
    _ = registry, environments, require_g1c_rows
    return []


def phase1c_gate_report_errors(report: dict) -> list[str]:
    """Fail closed on Phase 1C qualifying report invariants beyond JSON Schema."""
    _ = report
    return []


class GateValidatorTests(unittest.TestCase):
    def prepare_root(self, root: Path) -> None:
        (root / "environments").mkdir()
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


class Phase1cGateContractTests(unittest.TestCase):
    """RED contract tests for G1C-SEC registry rows and phase-1c gate reports."""

    def _metric_value(self, metric: str) -> int | float:
        operator, threshold = PHASE1C_METRIC_THRESHOLDS[metric]
        if operator in {">=", ">"}:
            return threshold
        if metric == "admin_mutation_audit_coverage_ratio":
            return 1.0
        if metric == "worker_dedicated_role_verified":
            return 1
        return 0

    def _threshold_decisions(self) -> list[dict]:
        return [
            {
                "metric": metric,
                "operator": operator,
                "value": value,
                "source": "docs/markhand-web-sla-targets.md",
                "owner": "security-owner",
                "approver": "operations-owner",
                "approvedAt": "2026-08-04T00:00:00Z",
            }
            for metric, (operator, value) in PHASE1C_METRIC_THRESHOLDS.items()
        ]

    def _gate_results(self) -> list[dict]:
        results: list[dict] = []
        for row in G1C_GATE_ROWS:
            metric = str(row["metric"])
            operator, threshold = PHASE1C_METRIC_THRESHOLDS[metric]
            results.append(
                {
                    "gateId": row["id"],
                    "externalGate": G1C_GATE_FAMILY,
                    "metric": metric,
                    "value": self._metric_value(metric),
                    "pass": True,
                    "failureDisposition": PHASE1C_FAILURE_DISPOSITION,
                    "evidence": row["evidence"],
                }
            )
            _ = operator, threshold
        return results

    def _baseline_report(self) -> dict:
        return {
            "version": 1,
            "reportId": "phase1c-gate-contract",
            "generatedAt": "2026-08-04T00:00:00Z",
            "status": "pass",
            "command": "python3 bench/markhand_web/scripts/run_phase1c_gate.py",
            "environmentId": PHASE1C_ENVIRONMENT_ID,
            "workloadProfileId": PHASE1C_WORKLOAD_PROFILE_ID,
            "targetMatch": True,
            "denialManifestSha256": "0" * 64,
            "git": {"commit": "0" * 40, "dirty": False},
            "metrics": {
                metric: self._metric_value(metric)
                for metric in PHASE1C_METRIC_THRESHOLDS
            },
            "thresholdDecisions": self._threshold_decisions(),
            "workerProof": {
                "runtimeRole": "markhand_worker",
                "dedicatedDatabaseUrlVerified": True,
                "superuser": False,
                "bypassRls": False,
                "verifiedAt": "2026-08-04T00:00:00Z",
            },
            "gateResults": self._gate_results(),
            "redactionScan": {"passed": True},
            "vulnerabilityScan": {
                "scanner": "trivy",
                "undispositionedHighCritical": 0,
                "findings": [],
            },
        }

    def _baseline_registry(self) -> dict:
        gates: list[dict] = []
        for row in G1C_GATE_ROWS:
            metric = str(row["metric"])
            operator, value = PHASE1C_METRIC_THRESHOLDS[metric]
            gates.append(
                {
                    "id": row["id"],
                    "externalGate": G1C_GATE_FAMILY,
                    "metric": {
                        "name": metric,
                        "unit": "count" if isinstance(value, int) else "milliseconds",
                        "statistic": "max" if operator in {"<=", "<"} else "min",
                    },
                    "workload": "loads.peak",
                    "corpus": None,
                    "threshold": {"operator": operator, "value": value},
                    "command": "python3 bench/markhand_web/scripts/run_phase1c_gate.py",
                    "environmentId": PHASE1C_ENVIRONMENT_ID,
                    "owner": row["owner"],
                    "approver": row["approver"],
                    "status": "approved",
                    "failureDisposition": PHASE1C_FAILURE_DISPOSITION,
                    "evidence": row["evidence"],
                    "blocksIssues": ["1C-13"],
                }
            )
        return {"version": 1, "registryStatus": "approved", "gates": gates}

    def _baseline_environment(self) -> dict:
        return {
            "version": 1,
            "environmentId": PHASE1C_ENVIRONMENT_ID,
            "role": "benchmark-target",
            "status": "approved",
            "approver": "project-owner",
            "approvedAt": "2026-08-04T00:00:00Z",
            "orgCount": 2,
            "embeddingProfile": "mock",
            "requiresDedicatedWorkerRole": True,
            "requiresWorkerDatabaseUrl": True,
            "cpu": {"vendor": "reference", "model": "single-host-x86_64", "cores": 8, "threads": 8},
            "ramGb": 16,
            "disk": {"type": "ssd", "capacityGb": 60, "iopsNote": ">=5k random-read IOPS"},
            "gpu": {"model": "none", "vramGb": 0, "count": 0},
            "network": {"bandwidthGbps": 1, "latencyMsAssumed": 1},
            "os": {"distro": "ubuntu-22.04", "arch": "x86_64"},
            "fingerprintRequiredFields": [
                "gitCommit",
                "workloadProfileId",
                "composeFileSha256",
                "imageIds",
                "indexSignature",
                "migrationManifestSha256",
                "hardware",
            ],
        }

    def test_phase1c_report_schema_rejects_missing_metrics_and_worker_proof(self) -> None:
        schema = load_json_yaml(PHASE1C_GATE_REPORT_SCHEMA)
        report = self._baseline_report()
        report.pop("metrics")
        report.pop("workerProof")
        errors = schema_errors(report, schema, "phase1c-report")
        self.assertTrue(any("metrics" in error for error in errors))
        self.assertTrue(any("workerProof" in error for error in errors))

    def test_phase1c_registry_requires_g1c_sec_family_and_environment(self) -> None:
        registry = self._baseline_registry()
        environments = [self._baseline_environment()]
        registry["gates"][0]["externalGate"] = "G0-SEC"
        environments[0]["environmentId"] = "poc-compose"
        errors = phase1c_registry_contract_errors(registry, environments)
        self.assertTrue(any("G1C-SEC" in error for error in errors))
        self.assertTrue(any(PHASE1C_ENVIRONMENT_ID in error for error in errors))

    def test_phase1c_registry_requires_all_metrics_block_phase_1c_and_evidence(self) -> None:
        registry = self._baseline_registry()
        environments = [self._baseline_environment()]
        registry["gates"] = registry["gates"][:-2]
        registry["gates"][0]["failureDisposition"] = "block-phase-1b"
        registry["gates"][1]["evidence"] = None
        errors = phase1c_registry_contract_errors(registry, environments)
        self.assertTrue(any("metric" in error.lower() for error in errors))
        self.assertTrue(any("block-phase-1c" in error for error in errors))
        self.assertTrue(any("evidence" in error for error in errors))

    def test_phase1c_registry_requires_approved_owner_and_approver(self) -> None:
        registry = self._baseline_registry()
        environments = [self._baseline_environment()]
        registry["gates"][0]["owner"] = ""
        registry["gates"][1]["approver"] = ""
        errors = phase1c_registry_contract_errors(registry, environments)
        self.assertTrue(any("owner" in error for error in errors))
        self.assertTrue(any("approver" in error for error in errors))

    def test_phase1c_report_rejects_pass_with_target_match_false(self) -> None:
        report = self._baseline_report()
        report["targetMatch"] = False
        errors = phase1c_gate_report_errors(report)
        self.assertTrue(any("targetMatch" in error for error in errors))

    def test_phase1c_report_rejects_missing_worker_proof(self) -> None:
        report = self._baseline_report()
        report.pop("workerProof")
        errors = phase1c_gate_report_errors(report)
        self.assertTrue(any("workerProof" in error for error in errors))

    def test_phase1c_report_rejects_missing_threshold_decision(self) -> None:
        report = self._baseline_report()
        report["thresholdDecisions"] = []
        errors = phase1c_gate_report_errors(report)
        self.assertTrue(any("thresholdDecisions" in error for error in errors))


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
        suite.addTests(loader.loadTestsFromTestCase(Phase1cGateContractTests))
        return 0 if unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful() else 1
    try:
        errors = validate(args.root)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"gate registry error: {error}", file=sys.stderr)
        return 1
    errors += phase1b_gate_report_errors(args.root / "reports/phase-1b-gate")
    if errors:
        print("gate registry validation failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    print("Markhand workload and gate registry valid")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
