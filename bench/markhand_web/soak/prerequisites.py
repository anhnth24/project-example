"""Fail-closed prerequisite validation for P1B-O05."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any


EXPECTED_POC_SERVICES = [
    "api",
    "minio",
    "postgres",
    "qdrant",
    "worker-convert",
    "worker-index",
]

RPO_SECONDS_MAX = 15 * 60
QUERY_READY_RTO_SECONDS_MAX = 60 * 60
FULL_VECTOR_RTO_SECONDS_MAX = 240 * 60


def _load_json(path: Path) -> dict[str, Any] | None:
    if not path.is_file():
        return None
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    return data if isinstance(data, dict) else None


def _raw_ok(report: dict[str, Any]) -> bool:
    raw = report.get("rawDir")
    if not isinstance(raw, str) or not raw:
        return False
    path = Path(raw)
    if not path.is_dir():
        return False
    try:
        return any(path.iterdir())
    except OSError:
        return False


def _git_full(report: dict[str, Any]) -> str | None:
    prov = report.get("provenance") if isinstance(report.get("provenance"), dict) else {}
    for key in ("gitShaFull", "git_sha_full"):
        val = report.get(key) or prov.get(key)
        if isinstance(val, str) and val.strip():
            return val.strip()
    return None


def _validate_f02(data: dict[str, Any] | None, path: Path, compose_project: str) -> list[str]:
    blockers: list[str] = []
    if data is None:
        return ["f02_missing"]
    if data.get("issue") != "P1B-F02":
        blockers.append("f02_issue_mismatch")
    if data.get("passed") is not True:
        blockers.append("f02_not_passed")
    project = data.get("composeProject") or data.get("compose_project")
    if project != compose_project:
        blockers.append("f02_compose_project_mismatch")
    image_ids = data.get("imageIds") or data.get("image_ids") or {}
    if not isinstance(image_ids, dict) or not image_ids:
        blockers.append("f02_missing_image_ids")
    else:
        for svc in EXPECTED_POC_SERVICES:
            if svc not in image_ids or not image_ids.get(svc):
                blockers.append(f"f02_missing_image:{svc}")
    if not _raw_ok(data) and not data.get("provenance"):
        # Accept either rawDir evidence or embedded provenance block.
        if not isinstance(data.get("provenance"), dict):
            # Some F02 reports use top-level provenance fields + rawDir.
            if not _raw_ok(data):
                # Allow missing rawDir only when containerIds+imageIds present (machine report).
                container_ids = data.get("containerIds") or {}
                if not isinstance(container_ids, dict) or not container_ids:
                    blockers.append("f02_missing_raw_or_provenance")
    return blockers


def _validate_o02(data: dict[str, Any] | None) -> list[str]:
    blockers: list[str] = []
    if data is None:
        return ["o02_missing"]
    if data.get("issue") != "P1B-O02":
        blockers.append("o02_issue_mismatch")
    # Alerts evidence passed: failCount==0, live fault executed, transitions ok.
    if data.get("failCount") not in (0, 0.0):
        blockers.append("o02_fail_count_nonzero")
    if data.get("liveFaultExecuted") is not True and data.get("status") != "pass":
        blockers.append("o02_alerts_evidence_not_passed")
    if data.get("status") == "fail":
        blockers.append("o02_status_fail")
    if data.get("status") == "pass" or (
        data.get("failCount") in (0, 0.0)
        and data.get("liveFaultExecuted") is True
        and int(data.get("passCount") or 0) > 0
    ):
        pass
    else:
        blockers.append("o02_alerts_evidence_not_passed")
    transitions = data.get("transitions") if isinstance(data.get("transitions"), dict) else {}
    if transitions:
        for name, row in transitions.items():
            if isinstance(row, dict) and row.get("ok") is False:
                blockers.append(f"o02_transition_failed:{name}")
    if not _raw_ok(data):
        blockers.append("o02_raw_missing")
    return blockers


def _validate_o03(data: dict[str, Any] | None) -> list[str]:
    blockers: list[str] = []
    if data is None:
        return ["o03_missing"]
    if data.get("issue") != "P1B-O03":
        blockers.append("o03_issue_mismatch")
    if data.get("consistencyRpoPass") is not True:
        blockers.append("o03_consistencyRpoPass_not_true")
    if data.get("queryReadyRtoPass") is not True:
        blockers.append("o03_queryReadyRtoPass_not_true")

    def measured(keys: tuple[str, ...]) -> float | None:
        for key in keys:
            val = data.get(key)
            if isinstance(val, (int, float)):
                return float(val)
        return None

    rpo = measured(("rpoSecondsMeasured", "captureWindowSeconds", "rpoSeconds"))
    q_rto = measured(
        ("queryReadyRtoSecondsMeasured", "restoreGreenSeconds", "queryReadyRtoSeconds")
    )
    full_rto = measured(("fullVectorRtoSecondsMeasured", "fullVectorRtoSeconds"))
    if rpo is None:
        blockers.append("o03_rpo_measured_missing")
    elif rpo > RPO_SECONDS_MAX:
        blockers.append("o03_rpo_exceeds_15m")
    if q_rto is None:
        blockers.append("o03_query_ready_rto_measured_missing")
    elif q_rto > QUERY_READY_RTO_SECONDS_MAX:
        blockers.append("o03_query_ready_rto_exceeds_60m")
    if full_rto is None:
        blockers.append("o03_full_vector_rto_measured_missing")
    elif full_rto > FULL_VECTOR_RTO_SECONDS_MAX:
        blockers.append("o03_full_vector_rto_exceeds_240m")
    if not _raw_ok(data):
        blockers.append("o03_raw_missing")
    return blockers


def _validate_o04(
    data: dict[str, Any] | None, compose_project: str
) -> list[str]:
    blockers: list[str] = []
    if data is None:
        return ["o04_missing"]
    if data.get("issue") != "P1B-O04":
        blockers.append("o04_issue_mismatch")
    if data.get("status") != "pass":
        blockers.append("o04_not_passed")
    prov = data.get("provenance") if isinstance(data.get("provenance"), dict) else {}
    if prov.get("composeProject") != compose_project:
        blockers.append("o04_compose_project_mismatch")
    image_ids = prov.get("imageIds") if isinstance(prov.get("imageIds"), dict) else {}
    for svc in EXPECTED_POC_SERVICES:
        if svc not in image_ids:
            blockers.append(f"o04_missing_image:{svc}")
    if not _raw_ok(data):
        blockers.append("o04_raw_missing")
    return blockers


def validate_prerequisites(
    *,
    f02_path: Path,
    o02_path: Path,
    o03_path: Path,
    o04_path: Path,
    current_git_full: str,
    compose_project: str,
) -> dict[str, Any]:
    """Validate F02/O02/O03/O04 evidence. Missing/null/stale/mismatched => non-pass."""
    blockers: list[str] = []
    f02 = _load_json(f02_path)
    o02 = _load_json(o02_path)
    o03 = _load_json(o03_path)
    o04 = _load_json(o04_path)

    blockers.extend(_validate_f02(f02, f02_path, compose_project))
    blockers.extend(_validate_o02(o02))
    blockers.extend(_validate_o03(o03))
    blockers.extend(_validate_o04(o04, compose_project))

    # Stale / mismatched git across prerequisites vs current HEAD.
    for label, data in (("f02", f02), ("o02", o02), ("o03", o03), ("o04", o04)):
        if data is None:
            continue
        git_full = _git_full(data)
        if git_full and current_git_full and git_full != current_git_full:
            blockers.append(f"stale_git:{label}")

    # Deduplicate while preserving order.
    seen: set[str] = set()
    uniq: list[str] = []
    for item in blockers:
        if item not in seen:
            seen.add(item)
            uniq.append(item)

    return {
        "ok": not uniq,
        "blockers": uniq,
        "f02": {"path": str(f02_path), "passed": f02.get("passed") if f02 else False},
        "o02": {"path": str(o02_path), "status": o02.get("status") if o02 else None},
        "o03": {
            "path": str(o03_path),
            "consistencyRpoPass": o03.get("consistencyRpoPass") if o03 else None,
            "queryReadyRtoPass": o03.get("queryReadyRtoPass") if o03 else None,
        },
        "o04": {"path": str(o04_path), "status": o04.get("status") if o04 else None},
    }
