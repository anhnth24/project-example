"""Fail-closed prerequisite validation for P1B-O05."""

from __future__ import annotations

import hashlib
import json
import math
import re
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
EXPECTED_REPORT_ISSUES = {
    "f02": "P1B-F02",
    "o02": "P1B-O02",
    "o03": "P1B-O03",
    "o04": "P1B-O04",
}

RPO_SECONDS_MAX = 15 * 60
QUERY_READY_RTO_SECONDS_MAX = 60 * 60
FULL_VECTOR_RTO_SECONDS_MAX = 240 * 60

ROOT = Path(__file__).resolve().parents[3]
COMPOSE_POC = ROOT / "deploy/compose.poc.yml"
MIGRATIONS_MANIFEST = ROOT / "crates/server/migrations/manifest.json"
REPORTS_ROOT = ROOT / "bench/markhand_web/reports"
EXPECTED_REPORT_PATHS = {
    "f02": REPORTS_ROOT / "poc-f02-boot.json",
    "o02": REPORTS_ROOT / "phase-1b-gate/o02-alerts.json",
    "o03": REPORTS_ROOT / "phase-1b-gate/o03-restore.json",
    "o04": REPORTS_ROOT / "phase-1b-gate/o04-release.json",
}
_GIT_SHA_RE = re.compile(r"^[0-9a-f]{40}$")
_HEX64_RE = re.compile(r"^[0-9a-f]{64}$")


def _load_json(path: Path) -> dict[str, Any] | None:
    if not path.is_file():
        return None
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    return data if isinstance(data, dict) else None


def _file_sha256(path: Path) -> str | None:
    if not path.is_file():
        return None
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _inside(child: Path, parent: Path) -> bool:
    try:
        child.resolve().relative_to(parent.resolve())
        return True
    except (OSError, ValueError):
        return False


def _is_number(value: Any) -> bool:
    return (
        not isinstance(value, bool)
        and isinstance(value, (int, float))
        and math.isfinite(float(value))
        and float(value) >= 0.0
    )


def _resolve_raw_dir(
    report: dict[str, Any],
    *,
    report_path: Path | None = None,
    allow_external: bool = False,
) -> Path | None:
    raw = report.get("rawDir")
    if not isinstance(raw, str) or not raw:
        return None
    path = Path(raw)
    if not path.is_absolute():
        candidates = [ROOT / path]
        if report_path is not None:
            candidates.append(report_path.resolve().parent / path)
        path = next(
            (candidate for candidate in candidates if candidate.is_dir()), candidates[0]
        )
    if not path.is_dir() or (not allow_external and not _inside(path, REPORTS_ROOT)):
        return None
    return path.resolve()


def _resolve_raw_manifest(
    report: dict[str, Any],
    raw_dir: Path,
    *,
    report_path: Path | None = None,
) -> Path:
    manifest_path = raw_dir / "raw-manifest.json"
    manifest_ref = report.get("rawArtifactManifest")
    if not manifest_path.is_file() and isinstance(manifest_ref, dict):
        configured = manifest_ref.get("path")
        if isinstance(configured, str) and configured:
            configured_path = Path(configured)
            if configured_path.is_absolute():
                candidates = [configured_path]
            else:
                candidates = [ROOT / configured_path]
                if report_path is not None:
                    candidates.append(report_path.resolve().parent / configured_path)
            manifest_path = next(
                (candidate for candidate in candidates if candidate.is_file()),
                candidates[0],
            )
    return manifest_path


def _raw_ok(
    report: dict[str, Any],
    *,
    report_path: Path | None = None,
    allow_external: bool = False,
) -> bool:
    path = _resolve_raw_dir(
        report,
        report_path=report_path,
        allow_external=allow_external,
    )
    if path is None:
        return False
    manifest_path = _resolve_raw_manifest(
        report,
        path,
        report_path=report_path,
    )
    if not _inside(manifest_path, path):
        return False
    if not manifest_path.is_file():
        return False
    manifest_ref = report.get("rawArtifactManifest")
    if not isinstance(manifest_ref, dict):
        return False
    manifest_sha = manifest_ref.get("sha256")
    if not isinstance(manifest_sha, str) or not _HEX64_RE.fullmatch(manifest_sha):
        return False
    if _file_sha256(manifest_path) != manifest_sha:
        return False
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return False
    files = manifest.get("files") if isinstance(manifest, dict) else None
    artifacts = manifest.get("artifacts") if isinstance(manifest, dict) else None
    if isinstance(files, list):
        rows = files
    elif isinstance(files, dict):
        rows = [
            {
                "path": rel,
                "sha256": (info.get("sha256") if isinstance(info, dict) else info),
            }
            for rel, info in files.items()
            if isinstance(rel, str)
        ]
    elif isinstance(artifacts, dict):
        rows = [
            {"path": rel, **info}
            for rel, info in artifacts.items()
            if isinstance(rel, str) and isinstance(info, dict)
        ]
    else:
        rows = []
    if not rows:
        return False
    for row in rows:
        if not isinstance(row, dict):
            return False
        rel = row.get("path")
        sha = row.get("sha256")
        if (
            not isinstance(rel, str)
            or Path(rel).is_absolute()
            or ".." in Path(rel).parts
        ):
            return False
        target = path / rel
        if (
            not _inside(target, path)
            or not target.is_file()
            or _file_sha256(target) != sha
        ):
            return False
    return True


def current_deploy_fingerprint() -> dict[str, str | None]:
    """Immutable deploy provenance used for compatibility checks."""
    migration = (
        hashlib.sha256(MIGRATIONS_MANIFEST.read_bytes()).hexdigest()
        if MIGRATIONS_MANIFEST.is_file()
        else None
    )
    compose = (
        hashlib.sha256(COMPOSE_POC.read_bytes()).hexdigest()
        if COMPOSE_POC.is_file()
        else None
    )
    return {
        "migrationManifestSha256": migration,
        "composeFileSha256": compose,
    }


def _image_ids(report: dict[str, Any]) -> dict[str, str]:
    prov = (
        report.get("provenance") if isinstance(report.get("provenance"), dict) else {}
    )
    images = (
        report.get("imageIds") or prov.get("imageIds") or report.get("image_ids") or {}
    )
    return images if isinstance(images, dict) else {}


def _prov_field(report: dict[str, Any], key: str) -> Any:
    prov = (
        report.get("provenance") if isinstance(report.get("provenance"), dict) else {}
    )
    if key in report and report.get(key) not in (None, ""):
        return report.get(key)
    return prov.get(key)


def _status_value(label: str, report: dict[str, Any]) -> Any:
    if label == "f02":
        return report.get("passed")
    if label == "o02":
        return report.get("status") or {
            "failCount": report.get("failCount"),
            "passCount": report.get("passCount"),
        }
    return report.get("status")


def _required_provenance(
    *,
    label: str,
    report: dict[str, Any] | None,
    current_git_full: str,
    compose_project: str,
    fingerprint: dict[str, str | None],
    live_image_ids: dict[str, str] | None,
    live_index_signature: str | None,
) -> list[str]:
    blockers: list[str] = []
    if report is None:
        return blockers
    git_sha = _prov_field(report, "gitShaFull")
    if not isinstance(git_sha, str) or not _GIT_SHA_RE.match(git_sha):
        blockers.append(f"provenance_missing:{label}:gitShaFull")
    elif git_sha != current_git_full:
        blockers.append(f"provenance_git_mismatch:{label}")
    if label == "f02":
        worktree = (
            report.get("gitWorktree")
            if isinstance(report.get("gitWorktree"), dict)
            else {}
        )
        git_dirty = worktree.get("dirty")
    else:
        provenance = (
            report.get("provenance")
            if isinstance(report.get("provenance"), dict)
            else {}
        )
        git_dirty = provenance.get("gitDirty")
    if git_dirty is not False:
        blockers.append(f"provenance_dirty_or_unknown:{label}")

    project = _prov_field(report, "composeProject")
    if project != compose_project:
        blockers.append(f"provenance_compose_project_mismatch:{label}")

    migration = _prov_field(report, "migrationManifestSha256")
    if not isinstance(migration, str) or not migration:
        blockers.append(f"provenance_missing:{label}:migrationManifestSha256")
    elif (
        fingerprint.get("migrationManifestSha256")
        and migration != fingerprint["migrationManifestSha256"]
    ):
        blockers.append(f"stale_incompatible:{label}:migrationManifestSha256")

    compose_hash = _prov_field(report, "composeFileSha256")
    if not isinstance(compose_hash, str) or not compose_hash:
        blockers.append(f"provenance_missing:{label}:composeFileSha256")
    elif (
        fingerprint.get("composeFileSha256")
        and compose_hash != fingerprint["composeFileSha256"]
    ):
        blockers.append(f"stale_incompatible:{label}:composeFileSha256")

    index_sig = _prov_field(report, "indexSignature")
    if not isinstance(index_sig, str) or not _HEX64_RE.match(index_sig):
        blockers.append(f"provenance_missing:{label}:indexSignature")
    elif live_index_signature and index_sig != live_index_signature:
        blockers.append(f"stale_incompatible:{label}:indexSignature")

    images = _image_ids(report)
    if not images:
        blockers.append(f"provenance_missing:{label}:imageIds")
    else:
        for svc in EXPECTED_POC_SERVICES:
            image = images.get(svc)
            if not image:
                blockers.append(f"provenance_missing:{label}:image:{svc}")
            elif (
                live_image_ids
                and live_image_ids.get(svc)
                and live_image_ids[svc] != image
            ):
                blockers.append(f"stale_incompatible:{label}:image:{svc}")
    return blockers


def _validate_f02(
    data: dict[str, Any] | None,
    compose_project: str,
    *,
    report_path: Path | None = None,
    allow_external_raw: bool = False,
) -> list[str]:
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
        blockers.append("f02_missing_container_ids")
    if not _raw_ok(
        data,
        report_path=report_path,
        allow_external=allow_external_raw,
    ):
        blockers.append("f02_raw_missing")
    return blockers


def _validate_o02(
    data: dict[str, Any] | None,
    *,
    report_path: Path | None = None,
    allow_external_raw: bool = False,
) -> list[str]:
    blockers: list[str] = []
    if data is None:
        return ["o02_missing"]
    if data.get("issue") != "P1B-O02":
        blockers.append("o02_issue_mismatch")
    if not _is_number(data.get("failCount")):
        blockers.append("o02_fail_count_invalid")
    elif float(data.get("failCount")) != 0.0:
        blockers.append("o02_fail_count_nonzero")
    if data.get("status") == "fail":
        blockers.append("o02_status_fail")
    if data.get("status") == "pass" or (
        data.get("failCount") in (0, 0.0)
        and data.get("liveFaultExecuted") is True
        and _is_number(data.get("passCount"))
        and int(data.get("passCount") or 0) > 0
    ):
        pass
    else:
        blockers.append("o02_alerts_evidence_not_passed")
    transitions = (
        data.get("transitions") if isinstance(data.get("transitions"), dict) else {}
    )
    if transitions:
        for name, row in transitions.items():
            if isinstance(row, dict) and row.get("ok") is False:
                blockers.append(f"o02_transition_failed:{name}")
    if not _raw_ok(
        data,
        report_path=report_path,
        allow_external=allow_external_raw,
    ):
        blockers.append("o02_raw_missing")
    return blockers


def _validate_o03(
    data: dict[str, Any] | None,
    *,
    report_path: Path | None = None,
    allow_external_raw: bool = False,
) -> list[str]:
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
            if _is_number(val):
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
    if not _raw_ok(
        data,
        report_path=report_path,
        allow_external=allow_external_raw,
    ):
        blockers.append("o03_raw_missing")
    return blockers


def _validate_o04(
    data: dict[str, Any] | None,
    compose_project: str,
    *,
    report_path: Path | None = None,
    allow_external_raw: bool = False,
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
    image_ids = _image_ids(data)
    for svc in EXPECTED_POC_SERVICES:
        if svc not in image_ids:
            blockers.append(f"o04_missing_image:{svc}")
    if not _raw_ok(
        data,
        report_path=report_path,
        allow_external=allow_external_raw,
    ):
        blockers.append("o04_raw_missing")
    return blockers


def _provenance_compatible(
    *,
    reports: dict[str, dict[str, Any] | None],
    live_image_ids: dict[str, str] | None,
    live_index_signature: str | None,
    fingerprint: dict[str, str | None],
) -> list[str]:
    """Reject stale incompatible evidence across prerequisite reports."""
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

    return blockers


def validate_prerequisites(
    *,
    f02_path: Path,
    o02_path: Path,
    o03_path: Path,
    o04_path: Path,
    current_git_full: str,
    compose_project: str,
    current_git_clean: bool = False,
    live_image_ids: dict[str, str] | None = None,
    live_index_signature: str | None = None,
    trusted_attestation: bool = False,
) -> dict[str, Any]:
    """Validate F02/O02/O03/O04 evidence. Missing/null/incompatible => non-pass."""
    blockers: list[str] = []
    f02 = _load_json(f02_path)
    o02 = _load_json(o02_path)
    o03 = _load_json(o03_path)
    o04 = _load_json(o04_path)

    if not isinstance(current_git_full, str) or not _GIT_SHA_RE.match(current_git_full):
        blockers.append("current_git_full_missing")
    if current_git_clean is not True:
        blockers.append("current_git_tree_dirty")
    if not compose_project:
        blockers.append("compose_project_missing")
    if not live_image_ids:
        blockers.append("live_image_ids_missing")
    else:
        for svc in EXPECTED_POC_SERVICES:
            if not live_image_ids.get(svc):
                blockers.append(f"live_image_missing:{svc}")
    if not isinstance(live_index_signature, str) or not _HEX64_RE.match(
        live_index_signature
    ):
        blockers.append("live_index_signature_missing")

    blockers.extend(
        _validate_f02(
            f02,
            compose_project,
            report_path=f02_path,
            allow_external_raw=trusted_attestation,
        )
    )
    blockers.extend(
        _validate_o02(
            o02,
            report_path=o02_path,
            allow_external_raw=trusted_attestation,
        )
    )
    blockers.extend(
        _validate_o03(
            o03,
            report_path=o03_path,
            allow_external_raw=trusted_attestation,
        )
    )
    blockers.extend(
        _validate_o04(
            o04,
            compose_project,
            report_path=o04_path,
            allow_external_raw=trusted_attestation,
        )
    )

    fingerprint = current_deploy_fingerprint()
    report_inputs = {
        "f02": (f02_path, f02),
        "o02": (o02_path, o02),
        "o03": (o03_path, o03),
        "o04": (o04_path, o04),
    }
    canonical_reports: dict[str, dict[str, Any]] = {}
    for label, (path, data) in report_inputs.items():
        expected_path = EXPECTED_REPORT_PATHS[label]
        try:
            if (
                path.resolve() != expected_path.resolve()
                and trusted_attestation is not True
            ):
                blockers.append(f"canonical_report_path_mismatch:{label}")
            if not _inside(path, REPORTS_ROOT) and trusted_attestation is not True:
                blockers.append(f"canonical_report_path_unsafe:{label}")
        except OSError:
            blockers.append(f"canonical_report_path_unresolved:{label}")
        resolved_raw = (
            _resolve_raw_dir(
                data,
                report_path=path,
                allow_external=trusted_attestation,
            )
            if data
            else None
        )
        canonical_reports[label] = {
            "path": str(path),
            "sha256": _file_sha256(path),
            "issue": data.get("issue") if data else None,
            "status": _status_value(label, data) if data else None,
            "rawDir": str(resolved_raw) if resolved_raw else None,
            "rawManifestSha256": (
                _file_sha256(
                    _resolve_raw_manifest(
                        data,
                        resolved_raw,
                        report_path=path,
                    )
                )
                if resolved_raw and data
                else None
            ),
            "trustedAttestation": bool(trusted_attestation),
        }
        if data is not None and data.get("issue") != EXPECTED_REPORT_ISSUES[label]:
            blockers.append(f"canonical_report_issue_mismatch:{label}")
        if data is not None and not _file_sha256(path):
            blockers.append(f"canonical_report_hash_missing:{label}")
        blockers.extend(
            _required_provenance(
                label=label,
                report=data,
                current_git_full=current_git_full,
                compose_project=compose_project,
                fingerprint=fingerprint,
                live_image_ids=live_image_ids,
                live_index_signature=live_index_signature,
            )
        )
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
        "canonicalReports": canonical_reports,
        "f02": {"path": str(f02_path), "passed": f02.get("passed") if f02 else False},
        "o02": {"path": str(o02_path), "status": o02.get("status") if o02 else None},
        "o03": {
            "path": str(o03_path),
            "consistencyRpoPass": o03.get("consistencyRpoPass") if o03 else None,
            "queryReadyRtoPass": o03.get("queryReadyRtoPass") if o03 else None,
        },
        "o04": {"path": str(o04_path), "status": o04.get("status") if o04 else None},
    }
