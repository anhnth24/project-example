"""Fail-closed prerequisite validation for P1B-O05."""

from __future__ import annotations

import hashlib
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

ROOT = Path(__file__).resolve().parents[3]
COMPOSE_POC = ROOT / "deploy/compose.poc.yml"
MIGRATIONS_MANIFEST = ROOT / "crates/server/migrations/manifest.json"


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


def current_deploy_fingerprint() -> dict[str, str | None]:
    """Immutable deploy provenance used for compatibility checks."""
    migration = (
        hashlib.sha256(MIGRATIONS_MANIFEST.read_bytes()).hexdigest()
        if MIGRATIONS_MANIFEST.is_file()
        else None
    )
    compose = (
        hashlib.sha256(COMPOSE_POC.read_bytes()).hexdigest() if COMPOSE_POC.is_file() else None
    )
    return {
        "migrationManifestSha256": migration,
        "composeFileSha256": compose,
    }


def _image_ids(report: dict[str, Any]) -> dict[str, str]:
    prov = report.get("provenance") if isinstance(report.get("provenance"), dict) else {}
    images = report.get("imageIds") or prov.get("imageIds") or report.get("image_ids") or {}
    return images if isinstance(images, dict) else {}


def _prov_field(report: dict[str, Any], key: str) -> Any:
    prov = report.get("provenance") if isinstance(report.get("provenance"), dict) else {}
    if key in report and report.get(key) not in (None, ""):
        return report.get(key)
    return prov.get(key)


def _validate_f02(data: dict[str, Any] | None, compose_project: str) -> list[str]:
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
    image_ids = _image_ids(data)
    if not image_ids:
        blockers.append("f02_missing_image_ids")
    else:
        for svc in EXPECTED_POC_SERVICES:
            if svc not in image_ids or not image_ids.get(svc):
                blockers.append(f"f02_missing_image:{svc}")
    container_ids = data.get("containerIds") or {}
    if not isinstance(container_ids, dict) or not container_ids:
        if not _raw_ok(data) and not isinstance(data.get("provenance"), dict):
            blockers.append("f02_missing_raw_or_provenance")
    return blockers


def _validate_o02(data: dict[str, Any] | None) -> list[str]:
    blockers: list[str] = []
    if data is None:
        return ["o02_missing"]
    if data.get("issue") != "P1B-O02":
        blockers.append("o02_issue_mismatch")
    if data.get("failCount") not in (0, 0.0):
        blockers.append("o02_fail_count_nonzero")
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


def _validate_o04(data: dict[str, Any] | None, compose_project: str) -> list[str]:
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
    image_ids = _image_ids(data)
    for svc in EXPECTED_POC_SERVICES:
        if svc not in image_ids:
            blockers.append(f"o04_missing_image:{svc}")
    if not _raw_ok(data):
        blockers.append("o04_raw_missing")
    return blockers


def _provenance_compatible(
    *,
    reports: dict[str, dict[str, Any] | None],
    live_image_ids: dict[str, str] | None,
    live_index_signature: str | None,
    fingerprint: dict[str, str | None],
) -> list[str]:
    """Reject stale incompatible evidence; allow ancestor git SHAs when deploy matches."""
    blockers: list[str] = []
    ref_images: dict[str, str] | None = None
    ref_migration: str | None = None
    ref_compose_hash: str | None = None
    ref_index: str | None = None

    for label, data in reports.items():
        if data is None:
            continue
        images = _image_ids(data)
        if images:
            if ref_images is None:
                ref_images = {k: str(v) for k, v in images.items() if v}
            else:
                for svc, image_id in ref_images.items():
                    if svc in images and str(images[svc]) != image_id:
                        blockers.append(f"provenance_image_mismatch:{label}:{svc}")
        migration = _prov_field(data, "migrationManifestSha256")
        if isinstance(migration, str) and migration:
            if ref_migration is None:
                ref_migration = migration
            elif migration != ref_migration:
                blockers.append(f"provenance_migration_mismatch:{label}")
        compose_hash = _prov_field(data, "composeFileSha256")
        if isinstance(compose_hash, str) and compose_hash:
            if ref_compose_hash is None:
                ref_compose_hash = compose_hash
            elif compose_hash != ref_compose_hash:
                blockers.append(f"provenance_compose_mismatch:{label}")
        index_sig = _prov_field(data, "indexSignature")
        if isinstance(index_sig, str) and index_sig:
            if ref_index is None:
                ref_index = index_sig
            elif index_sig != ref_index:
                blockers.append(f"provenance_index_mismatch:{label}")

    # Compare against live/current deploy fingerprint when available.
    cur_mig = fingerprint.get("migrationManifestSha256")
    if ref_migration and cur_mig and ref_migration != cur_mig:
        blockers.append("stale_incompatible:migrationManifestSha256")
    cur_compose = fingerprint.get("composeFileSha256")
    if ref_compose_hash and cur_compose and ref_compose_hash != cur_compose:
        blockers.append("stale_incompatible:composeFileSha256")

    if live_image_ids and ref_images:
        for svc, image_id in ref_images.items():
            if svc in live_image_ids and live_image_ids[svc] != image_id:
                blockers.append(f"stale_incompatible:image:{svc}")

    if live_index_signature and ref_index and live_index_signature != ref_index:
        blockers.append("stale_incompatible:indexSignature")

    # If reports carry image ids but they are empty/missing vs expected services → already covered.
    # Do NOT reject solely because gitSha differs from HEAD.
    return blockers


def validate_prerequisites(
    *,
    f02_path: Path,
    o02_path: Path,
    o03_path: Path,
    o04_path: Path,
    current_git_full: str,
    compose_project: str,
    live_image_ids: dict[str, str] | None = None,
    live_index_signature: str | None = None,
) -> dict[str, Any]:
    """Validate F02/O02/O03/O04 evidence. Missing/null/incompatible => non-pass."""
    del current_git_full  # retained for API compatibility; git SHA alone is not binding
    blockers: list[str] = []
    f02 = _load_json(f02_path)
    o02 = _load_json(o02_path)
    o03 = _load_json(o03_path)
    o04 = _load_json(o04_path)

    blockers.extend(_validate_f02(f02, compose_project))
    blockers.extend(_validate_o02(o02))
    blockers.extend(_validate_o03(o03))
    blockers.extend(_validate_o04(o04, compose_project))

    fingerprint = current_deploy_fingerprint()
    blockers.extend(
        _provenance_compatible(
            reports={"f02": f02, "o02": o02, "o03": o03, "o04": o04},
            live_image_ids=live_image_ids,
            live_index_signature=live_index_signature,
            fingerprint=fingerprint,
        )
    )

    seen: set[str] = set()
    uniq: list[str] = []
    for item in blockers:
        if item not in seen:
            seen.add(item)
            uniq.append(item)

    return {
        "ok": not uniq,
        "blockers": uniq,
        "fingerprint": fingerprint,
        "f02": {"path": str(f02_path), "passed": f02.get("passed") if f02 else False},
        "o02": {"path": str(o02_path), "status": o02.get("status") if o02 else None},
        "o03": {
            "path": str(o03_path),
            "consistencyRpoPass": o03.get("consistencyRpoPass") if o03 else None,
            "queryReadyRtoPass": o03.get("queryReadyRtoPass") if o03 else None,
        },
        "o04": {"path": str(o04_path), "status": o04.get("status") if o04 else None},
    }
