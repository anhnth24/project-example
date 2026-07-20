#!/usr/bin/env python3
"""Aggregate Phase-1B qualification gate report.

The report evaluates only target-valid evidence. Missing evidence, synthetic
smoke evidence, and targetMatch=false evidence remain pending with null measured
values so the report is safe to render before real infrastructure exists.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import string
import subprocess
import sys
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[3]
CORPUS = ROOT / "bench/markhand_web"
GATES_PATH = CORPUS / "gates.yaml"
SUMMARY_PATH = CORPUS / "reports/phase-1b-gate/summary.json"
REPORT_PATH = CORPUS / "reports/phase-1b-gate/report.md"
TEMPLATE_PATH = CORPUS / "reports/phase-1b-gate/template.md"
DEFAULT_EVIDENCE = {
    "soak": "bench/markhand_web/soak/summary.json",
    "query_load": "bench/markhand_web/query_load/summary.json",
    "ingest": "bench/markhand_web/ingest/summary.json",
    "restore": "bench/markhand_web/restore/summary.json",
}
SUPPORTED_GATE_IDS = {
    "G0-SLO-QUERY-P95",
    "G0-SLO-QUERY-P99",
    "G0-CAP-INGEST-THROUGHPUT",
    "G0-DR-RPO",
    "G0-DR-QUERY-READY-RTO",
    "G0-DR-FULL-VECTOR-RTO",
}


class ReportError(RuntimeError):
    """Actionable report generation error."""


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z")


def relative(path: Path) -> str:
    try:
        return str(path.resolve().relative_to(ROOT))
    except ValueError:
        return str(path)


def load_json(path: Path) -> dict[str, Any]:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ReportError(f"{relative(path)}: cannot load JSON-compatible YAML: {error}") from error
    if not isinstance(payload, dict):
        raise ReportError(f"{relative(path)}: expected object")
    return payload


def load_optional(path: Path) -> dict[str, Any] | None:
    if not path.is_file():
        return None
    return load_json(path)


def git(*args: str) -> str:
    try:
        return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def git_status() -> dict[str, Any]:
    raw = ""
    try:
        raw = subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT, text=True)
    except (OSError, subprocess.CalledProcessError):
        pass
    dirty_paths = []
    for line in raw.splitlines():
        if len(line) < 4:
            continue
        path = line[3:]
        if " -> " in path:
            path = path.split(" -> ", 1)[1]
        if path.startswith('"') and path.endswith('"'):
            path = path[1:-1]
        dirty_paths.append(path)
    return {
        "commit": git("rev-parse", "HEAD"),
        "branch": git("branch", "--show-current"),
        "dirty": bool(dirty_paths),
        "dirtyPaths": dirty_paths,
    }


def get_path(payload: dict[str, Any] | None, dotted: str) -> Any:
    current: Any = payload
    for part in dotted.split("."):
        if not isinstance(current, dict) or part not in current:
            return None
        current = current[part]
    return current


def evidence_target_valid(payload: dict[str, Any] | None) -> bool:
    if not isinstance(payload, dict):
        return False
    if payload.get("targetResultsValidForGate") is True:
        return True
    return payload.get("targetMatch") is True and payload.get("targetResultsValidForGate") is not False


def numeric(value: Any) -> float | None:
    if isinstance(value, bool) or value is None:
        return None
    if isinstance(value, (int, float)):
        return float(value)
    try:
        return float(str(value))
    except (TypeError, ValueError):
        return None


def compare(value: float, threshold: dict[str, Any]) -> bool:
    operator = threshold.get("operator")
    target = numeric(threshold.get("value"))
    if target is None:
        return False
    if operator == ">=":
        return value >= target
    if operator == "<=":
        return value <= target
    if operator == "==":
        return value == target
    raise ReportError(f"unsupported threshold operator: {operator}")


def candidate(
    source_name: str,
    source_path: Path,
    payload: dict[str, Any] | None,
    path: str,
) -> dict[str, Any]:
    value = numeric(get_path(payload, path))
    return {
        "source": source_name,
        "path": relative(source_path),
        "jsonPath": path,
        "targetValid": evidence_target_valid(payload),
        "targetMatch": bool(payload.get("targetMatch")) if isinstance(payload, dict) else False,
        "rawValue": value,
        "reportId": payload.get("reportId") if isinstance(payload, dict) else None,
    }


def gate_candidates(gate_id: str, evidence: dict[str, tuple[Path, dict[str, Any] | None]]) -> list[dict[str, Any]]:
    soak_path, soak = evidence["soak"]
    query_path, query = evidence["query_load"]
    ingest_path, ingest = evidence["ingest"]
    restore_path, restore = evidence["restore"]
    if gate_id == "G0-SLO-QUERY-P95":
        return [
            candidate("soak", soak_path, soak, "operations.query_search.durationMs.p95"),
            candidate("query_load", query_path, query, "metrics.queryLatencyP95Ms"),
            candidate("query_load", query_path, query, "metrics.queryLatencyP95MsSynthetic"),
        ]
    if gate_id == "G0-SLO-QUERY-P99":
        return [
            candidate("soak", soak_path, soak, "operations.query_search.durationMs.p99"),
            candidate("query_load", query_path, query, "metrics.filteredQueryLatencyP99Ms"),
            candidate("query_load", query_path, query, "metrics.filteredQueryLatencyP99MsSynthetic"),
        ]
    if gate_id == "G0-CAP-INGEST-THROUGHPUT":
        return [
            candidate("soak", soak_path, soak, "operations.ingest.successfulDocumentsPerHour"),
            candidate("ingest", ingest_path, ingest, "docsPerHour"),
            candidate("ingest", ingest_path, ingest, "headroomEstimate.effectiveCapacityDocsPerHourForGate"),
        ]
    if gate_id == "G0-DR-RPO":
        return [
            candidate("restore", restore_path, restore, "timings.rpoMinutes"),
            candidate("restore", restore_path, restore, "timings.rpoMinutesSynthetic"),
        ]
    if gate_id == "G0-DR-QUERY-READY-RTO":
        return [
            candidate("restore", restore_path, restore, "timings.queryReadyRtoMinutes"),
            candidate("restore", restore_path, restore, "timings.queryReadyRtoMinutesSynthetic"),
        ]
    if gate_id == "G0-DR-FULL-VECTOR-RTO":
        return [
            candidate("restore", restore_path, restore, "timings.fullVectorRtoMinutes"),
            candidate("restore", restore_path, restore, "timings.fullVectorRtoMinutesSynthetic"),
        ]
    return []


def evaluate_gate(gate: dict[str, Any], evidence: dict[str, tuple[Path, dict[str, Any] | None]]) -> dict[str, Any]:
    gate_id = str(gate.get("id"))
    candidates = gate_candidates(gate_id, evidence)
    valid = [item for item in candidates if item["targetValid"] and item["rawValue"] is not None]
    selected = valid[0] if valid else None
    threshold = gate.get("threshold", {})
    measured = selected["rawValue"] if selected else None
    if measured is None:
        status = "pending"
        reason = pending_reason(gate_id, candidates)
    else:
        passed = compare(float(measured), threshold)
        status = "pass" if passed else "fail"
        reason = "target-valid evidence evaluated"
    return {
        "gateId": gate_id,
        "externalGate": gate.get("externalGate"),
        "metric": gate.get("metric"),
        "threshold": threshold,
        "status": status,
        "measuredValue": measured,
        "evidence": selected,
        "candidateEvidence": candidates,
        "pendingReason": reason if status == "pending" else None,
        "supportedByPhase1BReport": gate_id in SUPPORTED_GATE_IDS,
    }


def pending_reason(gate_id: str, candidates: list[dict[str, Any]]) -> str:
    if gate_id not in SUPPORTED_GATE_IDS:
        return "not part of the P1B-O05 numeric soak/query/ingest/restore evidence set"
    if not candidates:
        return "no candidate evidence mapping"
    if not any(item["rawValue"] is not None for item in candidates):
        return "evidence absent or missing measured value"
    if not any(item["targetValid"] for item in candidates):
        return "evidence is present but not target-valid (targetMatch=false or targetResultsValidForGate=false)"
    return "target-valid evidence missing numeric value"


def build_payload(args: argparse.Namespace) -> dict[str, Any]:
    gates = load_json(args.gates)
    gate_rows = gates.get("gates", [])
    if not isinstance(gate_rows, list):
        raise ReportError("gates.yaml missing gates array")
    evidence: dict[str, tuple[Path, dict[str, Any] | None]] = {}
    for name, rel in DEFAULT_EVIDENCE.items():
        path = (args.evidence_root / rel).resolve()
        evidence[name] = (path, load_optional(path))
    evaluated = [evaluate_gate(gate, evidence) for gate in gate_rows if isinstance(gate, dict)]
    counts = {
        "pass": sum(1 for item in evaluated if item["status"] == "pass"),
        "fail": sum(1 for item in evaluated if item["status"] == "fail"),
        "pending": sum(1 for item in evaluated if item["status"] == "pending"),
    }
    all_supported_present = all(
        item["status"] == "pass" for item in evaluated if item["gateId"] in SUPPORTED_GATE_IDS
    )
    target_match = bool(evaluated) and counts["fail"] == 0 and counts["pending"] == 0 and all_supported_present
    return {
        "version": 1,
        "reportId": "phase-1b-gate-qualification",
        "generatedAt": utc_now(),
        "command": "python3 bench/markhand_web/reports/phase-1b-gate.py",
        "targetMatch": target_match,
        "statusCounts": counts,
        "git": git_status(),
        "gatesPath": relative(args.gates),
        "evidenceRoot": relative(args.evidence_root),
        "evidenceInputs": {
            name: {
                "path": relative(path),
                "present": payload is not None,
                "targetMatch": bool(payload.get("targetMatch")) if isinstance(payload, dict) else False,
                "targetResultsValidForGate": (
                    bool(payload.get("targetResultsValidForGate")) if isinstance(payload, dict) else False
                ),
                "reportId": payload.get("reportId") if isinstance(payload, dict) else None,
            }
            for name, (path, payload) in evidence.items()
        },
        "gates": evaluated,
        "caveat": (
            "Numeric G0-SLO/G0-CAP/DR/soak gates require sustained real infrastructure "
            "with targetMatch=true; pending/null rows are expected in this sandbox."
        ),
    }


def fmt(value: Any) -> str:
    if value is None:
        return "null"
    if isinstance(value, float):
        return f"{value:.3f}".rstrip("0").rstrip(".")
    return str(value)


def threshold_text(threshold: dict[str, Any]) -> str:
    return f"{threshold.get('operator')} {threshold.get('value')}"


def gate_rows_markdown(gates: list[dict[str, Any]]) -> str:
    lines = [
        "| gate id | metric | threshold | status | measured value | evidence | reason |",
        "|---|---|---:|---|---:|---|---|",
    ]
    for gate in gates:
        metric = gate.get("metric") or {}
        evidence = gate.get("evidence") or {}
        evidence_text = "none"
        if evidence:
            evidence_text = f"{evidence.get('source')}:{evidence.get('jsonPath')}"
        reason = gate.get("pendingReason") or "evaluated"
        lines.append(
            "| "
            f"`{gate['gateId']}` | `{metric.get('name')}` `{metric.get('statistic')}` | "
            f"`{threshold_text(gate.get('threshold') or {})}` | `{gate['status']}` | "
            f"`{fmt(gate.get('measuredValue'))}` | `{evidence_text}` | {reason} |"
        )
    return "\n".join(lines)


def evidence_inputs_markdown(inputs: dict[str, dict[str, Any]]) -> str:
    lines = [
        "| evidence | path | present | targetMatch | target-valid | report id |",
        "|---|---|---|---|---|---|",
    ]
    for name, item in inputs.items():
        lines.append(
            "| "
            f"`{name}` | `{item['path']}` | `{str(item['present']).lower()}` | "
            f"`{str(item['targetMatch']).lower()}` | `{str(item['targetResultsValidForGate']).lower()}` | "
            f"`{item['reportId']}` |"
        )
    return "\n".join(lines)


def render_report(payload: dict[str, Any], template_path: Path) -> str:
    if template_path.is_file():
        template_text = template_path.read_text(encoding="utf-8")
    else:
        template_text = DEFAULT_TEMPLATE
    template = string.Template(template_text)
    return template.safe_substitute(
        generatedAt=payload["generatedAt"],
        targetMatch=str(payload["targetMatch"]).lower(),
        passCount=payload["statusCounts"]["pass"],
        failCount=payload["statusCounts"]["fail"],
        pendingCount=payload["statusCounts"]["pending"],
        caveat=payload["caveat"],
        evidenceInputs=evidence_inputs_markdown(payload["evidenceInputs"]),
        gateRows=gate_rows_markdown(payload["gates"]),
        gitCommit=payload["git"]["commit"],
        gitDirty=str(payload["git"]["dirty"]).lower(),
    )


DEFAULT_TEMPLATE = """# Phase-1B gate qualification

- Generated: `$generatedAt`
- Git commit: `$gitCommit`
- Dirty at report time: `$gitDirty`
- `targetMatch`: `$targetMatch`
- Counts: pass `$passCount`, fail `$failCount`, pending `$pendingCount`

> $caveat

## Evidence inputs

$evidenceInputs

## Gates

$gateRows
"""


def write_outputs(payload: dict[str, Any], summary: Path, report: Path, template: Path) -> None:
    summary.parent.mkdir(parents=True, exist_ok=True)
    report.parent.mkdir(parents=True, exist_ok=True)
    summary.write_text(json.dumps(payload, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    report.write_text(render_report(payload, template), encoding="utf-8")


def self_test() -> None:
    assert compare(1.0, {"operator": ">=", "value": 1.0}) is True
    assert compare(2.0, {"operator": "<=", "value": 1.0}) is False
    row = evaluate_gate(
        {
            "id": "G0-SLO-QUERY-P95",
            "metric": {"name": "query_latency", "statistic": "p95"},
            "threshold": {"operator": "<=", "value": 500},
        },
        {
            "soak": (Path("/missing/soak.json"), None),
            "query_load": (Path("/missing/query.json"), None),
            "ingest": (Path("/missing/ingest.json"), None),
            "restore": (Path("/missing/restore.json"), None),
        },
    )
    assert row["status"] == "pending"
    assert row["measuredValue"] is None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--gates", type=Path, default=GATES_PATH)
    parser.add_argument("--evidence-root", type=Path, default=ROOT)
    parser.add_argument("--summary", type=Path, default=SUMMARY_PATH)
    parser.add_argument("--report", type=Path, default=REPORT_PATH)
    parser.add_argument("--template", type=Path, default=TEMPLATE_PATH)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    args.gates = args.gates.resolve()
    args.evidence_root = args.evidence_root.resolve()
    args.summary = args.summary.resolve()
    args.report = args.report.resolve()
    args.template = args.template.resolve()

    if args.self_test:
        self_test()
        print("self-test ok")
        return 0
    try:
        payload = build_payload(args)
        write_outputs(payload, args.summary, args.report, args.template)
    except ReportError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    print(f"wrote {relative(args.summary)}")
    print(f"wrote {relative(args.report)}")
    print(f"targetMatch={str(payload['targetMatch']).lower()}")
    print(f"pending={payload['statusCounts']['pending']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
