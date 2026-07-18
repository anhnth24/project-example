#!/usr/bin/env python3
"""Validate Markhand Web workload, hardware and decision-gate registry."""

from __future__ import annotations

import argparse
import json
import re
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_ROOT = ROOT / "bench/markhand_web"
SECRET_PATTERNS = (
    re.compile(r"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----"),
    re.compile(r"\bAKIA[0-9A-Z]{16}\b"),
    re.compile(r"\bpostgres(?:ql)?://[^/\s:@]+:[^@\s/]+@"),
    re.compile(r"(?:^|\s)/(?:home|Users|workspace|tmp)/\S+"),
    re.compile(r"\b[A-Za-z]:\\Users\\"),
)
GATE_FAMILIES = {"G0-ARCH", "G0-RET", "G0-SEC", "G0-CAP", "G0-SLO", "G0-LIC"}
OPERATORS = {">=", ">", "<=", "<", "=="}
FAILURE_DISPOSITIONS = {"block-phase-1b", "block-issue", "research-only", "waive-with-adr"}
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
    return isinstance(value, (int, float)) and not isinstance(value, bool) and value > 0


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
    errors = security_errors([workload_path, gates_path, *environment_paths])

    errors += required_fields(
        workload,
        ("version", "profileId", "status", "approver", "openDecisions", "scale", "loads", "workloads", "hardware"),
        "workload",
    )
    if workload.get("version") != 1:
        errors.append("workload: version must be 1")
    if workload.get("status") not in {"proposed", "approved"}:
        errors.append("workload: invalid status")
    for tier in ("normal", "peak", "recovery", "aggregate"):
        if not isinstance(workload.get("loads", {}).get(tier), dict):
            errors.append(f"workload: missing loads.{tier}")
    for decision in workload.get("openDecisions", []):
        errors += required_fields(decision, ("id", "question", "owner", "status", "blocks"), "decision")
        if decision.get("status") == "open" and not decision.get("owner"):
            errors.append(f"decision {decision.get('id')}: open decision requires owner")
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
                ("gpu.vramGb", environment.get("gpu", {}).get("vramGb")),
                ("gpu.count", environment.get("gpu", {}).get("count")),
                ("network.bandwidthGbps", environment.get("network", {}).get("bandwidthGbps")),
                ("network.latencyMsAssumed", environment.get("network", {}).get("latencyMsAssumed")),
            )
            for field, value in numeric_paths:
                if not positive_number(value):
                    errors.append(f"{source}: approved {field} must be positive")

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
        if status != "proposed" and not isinstance(threshold.get("value"), (int, float)):
            errors.append(f"gate {gate_id}: non-proposed threshold must be numeric")
        metric = gate.get("metric", {})
        if (
            metric.get("unit") == "ratio"
            and isinstance(threshold.get("value"), (int, float))
            and not 0 <= threshold["value"] <= 1
        ):
            errors.append(f"gate {gate_id}: ratio threshold must be within 0..1")
        if gate.get("failureDisposition") not in FAILURE_DISPOSITIONS:
            errors.append(f"gate {gate_id}: invalid failureDisposition")
        if not gate.get("owner") or not gate.get("approver") or not gate.get("command"):
            errors.append(f"gate {gate_id}: owner, approver, and command are required")
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
    return errors


class GateValidatorTests(unittest.TestCase):
    def test_repository_registry_is_valid(self) -> None:
        self.assertEqual(validate(DEFAULT_ROOT), [])

    def test_denies_duplicate_missing_approver_unknown_environment_and_secret(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "environments").mkdir()
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
            (root / "environments").mkdir()
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
            (root / "environments").mkdir()
            workload = load_json_yaml(DEFAULT_ROOT / "workload-profile.yaml")
            gates = load_json_yaml(DEFAULT_ROOT / "gates.yaml")
            environment = load_json_yaml(DEFAULT_ROOT / "environments/on-prem-reference.yaml")
            workload["openDecisions"][0]["status"] = "open"
            workload["scale"]["orgCount"] = 0
            environment["gpu"]["count"] = 0
            gates["gates"][0]["status"] = "proposed"
            (root / "workload-profile.yaml").write_text(json.dumps(workload))
            (root / "gates.yaml").write_text(json.dumps(gates))
            (root / "environments/on-prem-reference.yaml").write_text(json.dumps(environment))
            errors = validate(root)
            self.assertTrue(any("unresolved decisions" in error for error in errors))
            self.assertTrue(any("scale.orgCount must be positive" in error for error in errors))
            self.assertTrue(any("gpu.count must be positive" in error for error in errors))
            self.assertTrue(any("non-approved gates" in error for error in errors))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=DEFAULT_ROOT)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(GateValidatorTests)
        return 0 if unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful() else 1
    try:
        errors = validate(args.root)
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
