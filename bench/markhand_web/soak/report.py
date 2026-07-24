"""O05 report build / evaluate / write (canonical o05-soak.json)."""

from __future__ import annotations

import json
import hashlib
import os
import re
import tempfile
import time
from pathlib import Path
from typing import Any

import gates_eval
import redact


ISSUE = "P1B-O05"
CANONICAL = "o05-soak.json"
ROOT = Path(__file__).resolve().parents[3]
DEFAULT_OUT = ROOT / "bench/markhand_web/reports/phase-1b-gate"
_RAW_RUN_RE = re.compile(r"^o05-[0-9]{8}T[0-9]{6}Z(?:-[0-9a-f]{32})?$")
_NUMERIC_FIELDS = {
    "queryP50Ms",
    "queryP95Ms",
    "queryP99Ms",
    "querySuccessSamples",
    "ingestDocsPerHour",
    "ingestOk",
    "rssGrowthMb",
    "tempGrowthMb",
    "queueDepthMax",
    "queueAgeMaxSeconds",
    "dbConnectionsMax",
    "requestErrors",
    "requestErrorsOutsideInjection",
    "requestErrorsInInjection",
    "deletedCount",
    "retainedCount",
    "durationSeconds",
}
_HEX64_RE = re.compile(r"^[0-9a-f]{64}$")


def unknown_gates() -> dict[str, str]:
    return {
        "queryP95": "unknown",
        "queryP99": "unknown",
        "ingestThroughput": "unknown",
        "rssGrowth": "unknown",
        "tempGrowth": "unknown",
        "queueDepth": "unknown",
        "dbConnections": "unknown",
        "unboundedGrowth": "unknown",
        "recovery": "unknown",
        "postRestoreRetrieval": "unknown",
        "requestErrors": "unknown",
        "completeness": "unknown",
        "canonicalBinding": "unknown",
        "workloadDrain": "unknown",
        "reconcile": "unknown",
        "resourceCoverage": "unknown",
    }


def file_sha256(path: Path) -> str | None:
    if not path.is_file():
        return None
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _inside(child: Path, parent: Path) -> bool:
    try:
        child.resolve().relative_to(parent.resolve())
        return True
    except (OSError, ValueError):
        return False


def _canonical_raw_dir(raw_dir: Path, *, report_path: Path | None = None) -> tuple[bool, str | None]:
    try:
        resolved = raw_dir.resolve()
    except OSError:
        return False, "raw_dir_resolve_failed"
    base = (report_path.resolve().parent if report_path is not None else DEFAULT_OUT) / "raw"
    if not _inside(resolved, base):
        return False, "raw_dir_not_canonical"
    if resolved.parent.name != "raw" or not _RAW_RUN_RE.match(resolved.name):
        return False, "raw_dir_name_not_canonical"
    return True, None


def _is_finite_nonnegative_number(value: Any) -> bool:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return False
    return value == value and value not in (float("inf"), float("-inf")) and value >= 0


def _as_int(value: Any) -> int | None:
    if not _is_finite_nonnegative_number(value):
        return None
    if int(value) != float(value):
        return None
    return int(value)


def _validate_numeric_fields(payload: dict[str, Any]) -> list[str]:
    blockers: list[str] = []
    metrics = payload.get("metrics") if isinstance(payload.get("metrics"), dict) else {}
    for key in _NUMERIC_FIELDS:
        if key in metrics and metrics[key] is not None and not _is_finite_nonnegative_number(metrics[key]):
            blockers.append(f"metric_invalid:{key}")
    for key in ("durationSeconds", "officialDurationSeconds", "sampleIntervalSeconds"):
        if key in payload and payload[key] is not None and not _is_finite_nonnegative_number(payload[key]):
            blockers.append(f"payload_numeric_invalid:{key}")
    return blockers


def build_raw_manifest(raw_dir: Path) -> dict[str, Any]:
    files: list[dict[str, Any]] = []
    if not raw_dir.is_dir():
        return {"ok": False, "files": [], "error": "raw_dir_missing"}
    for path in sorted(raw_dir.rglob("*")):
        if not path.is_file() or path.name == "raw-manifest.json":
            continue
        rel = path.relative_to(raw_dir).as_posix()
        files.append({"path": rel, "sha256": file_sha256(path), "bytes": path.stat().st_size})
    return {"ok": bool(files), "files": files}


def write_raw_manifest(raw_dir: Path) -> dict[str, Any]:
    manifest = build_raw_manifest(raw_dir)
    (raw_dir / "raw-manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    manifest["path"] = str(raw_dir / "raw-manifest.json")
    manifest["sha256"] = file_sha256(raw_dir / "raw-manifest.json")
    return manifest


def _validate_raw_manifest(payload: dict[str, Any], *, report_path: Path | None) -> list[str]:
    blockers: list[str] = []
    raw_dir = Path(str(payload.get("rawDir") or ""))
    ok, reason = _canonical_raw_dir(raw_dir, report_path=report_path)
    if not ok:
        blockers.append(reason or "raw_dir_not_canonical")
    raw_manifest = payload.get("rawManifest") if isinstance(payload.get("rawManifest"), dict) else {}
    manifest_path = Path(str(raw_manifest.get("path") or ""))
    expected_manifest_path = raw_dir / "raw-manifest.json"
    try:
        if manifest_path.resolve() != expected_manifest_path.resolve():
            blockers.append("raw_manifest_path_not_canonical")
    except OSError:
        blockers.append("raw_manifest_path_not_canonical")
    if not raw_dir.is_dir() or not manifest_path.is_file():
        blockers.append("raw_manifest_missing")
        return blockers
    actual = file_sha256(manifest_path)
    if raw_manifest.get("sha256") != actual:
        blockers.append("raw_manifest_sha_mismatch")
    manifest_on_disk = None
    try:
        manifest_on_disk = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        blockers.append("raw_manifest_unreadable")
        return blockers
    if not isinstance(manifest_on_disk, dict):
        blockers.append("raw_manifest_not_object")
        return blockers
    declared = manifest_on_disk.get("files")
    if not isinstance(declared, list) or not declared:
        blockers.append("raw_manifest_empty")
        return blockers
    seen: set[str] = set()
    for row in declared:
        if not isinstance(row, dict):
            blockers.append("raw_manifest_entry_not_object")
            continue
        rel = row.get("path")
        sha = row.get("sha256")
        size = row.get("bytes")
        if not isinstance(rel, str) or not rel or Path(rel).is_absolute() or ".." in Path(rel).parts:
            blockers.append("raw_manifest_entry_path_unsafe")
            continue
        if rel in seen:
            blockers.append(f"raw_manifest_duplicate:{rel}")
        seen.add(rel)
        entry_path = raw_dir / rel
        if not _inside(entry_path, raw_dir) or not entry_path.is_file():
            blockers.append(f"raw_manifest_entry_missing:{rel}")
            continue
        if not isinstance(sha, str) or not _HEX64_RE.match(sha):
            blockers.append(f"raw_manifest_entry_sha_invalid:{rel}")
        elif file_sha256(entry_path) != sha:
            blockers.append(f"raw_manifest_entry_sha_mismatch:{rel}")
        if isinstance(size, bool) or not isinstance(size, int) or size < 0:
            blockers.append(f"raw_manifest_entry_size_invalid:{rel}")
        elif entry_path.stat().st_size != size:
            blockers.append(f"raw_manifest_entry_size_mismatch:{rel}")
    rebuilt = build_raw_manifest(raw_dir)
    declared_pairs = sorted((str(row.get("path")), str(row.get("sha256")), row.get("bytes")) for row in declared if isinstance(row, dict))
    rebuilt_pairs = sorted((str(row.get("path")), str(row.get("sha256")), row.get("bytes")) for row in rebuilt.get("files", []))
    if declared_pairs != rebuilt_pairs:
        blockers.append("raw_manifest_not_canonical")
    payload_manifest = {k: v for k, v in raw_manifest.items() if k in {"ok", "files"}}
    on_disk_manifest = {k: v for k, v in manifest_on_disk.items() if k in {"ok", "files"}}
    if payload_manifest != on_disk_manifest:
        blockers.append("raw_manifest_payload_mismatch")
    return blockers


def _validate_threshold_binding(payload: dict[str, Any]) -> tuple[dict[str, str], list[str]]:
    blockers: list[str] = []
    thresholds = payload.get("thresholds") if isinstance(payload.get("thresholds"), dict) else {}
    metrics = payload.get("metrics") if isinstance(payload.get("metrics"), dict) else {}
    if not thresholds:
        blockers.append("thresholds_missing")

    official_duration = _as_int(payload.get("officialDurationSeconds"))
    duration = _as_int(payload.get("durationSeconds"))
    threshold_official_duration = _as_int(thresholds.get("officialDurationSeconds"))
    profile_duration = _as_int(thresholds.get("profileDurationSeconds"))

    if official_duration != gates_eval.OFFICIAL_DURATION_SECONDS:
        blockers.append("official_duration_not_canonical")
    if duration != gates_eval.OFFICIAL_DURATION_SECONDS:
        blockers.append("duration_not_canonical")
    if threshold_official_duration != gates_eval.OFFICIAL_DURATION_SECONDS:
        blockers.append("threshold_official_duration_not_canonical")
    if profile_duration != gates_eval.OFFICIAL_DURATION_SECONDS:
        blockers.append("profile_duration_not_canonical")
    if thresholds.get("profileSha256") != gates_eval.CANONICAL_PROFILE_SHA256:
        blockers.append("profile_sha_not_canonical")
    if thresholds.get("gatesSha256") != gates_eval.CANONICAL_GATES_SHA256:
        blockers.append("gates_sha_not_canonical")
    if thresholds.get("canonicalThresholdValues") != gates_eval.CANONICAL_THRESHOLDS:
        blockers.append("threshold_values_not_canonical")
    for key, expected in gates_eval.CANONICAL_THRESHOLDS.items():
        if thresholds.get(key) != expected:
            blockers.append(f"threshold_value_mismatch:{key}")
    if thresholds.get("canonicalBindingPass") is not True:
        blockers.append("canonical_binding_not_passed")
    try:
        recomputed = gates_eval.evaluate_numeric_gates(metrics, thresholds or {})
    except (KeyError, TypeError, ValueError, RuntimeError, OverflowError):
        blockers.append("thresholds_incomplete")
        recomputed = unknown_gates()
    supplied = payload.get("gates") if isinstance(payload.get("gates"), dict) else {}
    if supplied != recomputed:
        blockers.append("gates_recomputed_mismatch")
    return recomputed, blockers


def validate_report_payload(payload: dict[str, Any], *, report_path: Path | None = None) -> tuple[str, list[str]]:
    """Re-evaluate canonical O05 JSON and raw manifest; do not trust stored status."""
    blockers: list[str] = []
    if payload.get("issue") != ISSUE:
        blockers.append("canonical_issue_mismatch")
    if payload.get("canonicalReport") != CANONICAL:
        blockers.append("canonical_report_mismatch")
    blockers.extend(_validate_numeric_fields(payload))
    blockers.extend(_validate_raw_manifest(payload, report_path=report_path))
    recomputed_gates, threshold_blockers = _validate_threshold_binding(payload)
    blockers.extend(threshold_blockers)
    if bool(payload.get("smokeNonQualifying") or payload.get("smoke")):
        blockers.append("smoke_non_qualifying_duration")
    if bool((payload.get("prerequisites") or {}).get("ok")) is not True:
        blockers.append("prerequisites_incomplete")

    failure = payload.get("failureInjection") if isinstance(payload.get("failureInjection"), dict) else {}
    summary = failure.get("summary") if isinstance(failure.get("summary"), dict) else {}
    metrics = payload.get("metrics") if isinstance(payload.get("metrics"), dict) else {}
    injection_ok = bool(
        failure.get("enabled")
        and summary.get("ok")
        and metrics.get("workerRecoveryPass") is True
        and metrics.get("dependencyRecoveryPass") is True
    )
    duration = _as_int(payload.get("durationSeconds"))
    status, eval_blockers = evaluate_status(
        markhand_soak=bool(payload.get("markhandSoak")),
        prerequisites_ok=bool((payload.get("prerequisites") or {}).get("ok")),
        measured=bool(metrics.get("measured")),
        smoke=bool(payload.get("smokeNonQualifying") or payload.get("smoke")),
        gates=recomputed_gates,
        injection_ok=injection_ok,
        redaction_ok=bool((payload.get("redactionScan") or {}).get("passed")),
        duration_seconds=duration if duration is not None else 0,
        official_duration=gates_eval.OFFICIAL_DURATION_SECONDS,
    )
    blockers.extend(eval_blockers)
    serialized = json.dumps(payload, sort_keys=True, ensure_ascii=False)
    residual = redact.scan_text(serialized)
    if residual:
        blockers.append("serialized_report_secret_scan_failed")
    if blockers and status != "not_run":
        status = "fail"
    return status, blockers


def evaluate_status(
    *,
    markhand_soak: bool,
    prerequisites_ok: bool,
    measured: bool,
    smoke: bool,
    gates: dict[str, str],
    injection_ok: bool,
    redaction_ok: bool,
    duration_seconds: int | float,
    official_duration: int,
) -> tuple[str, list[str]]:
    """Fail-closed status evaluation. Pass only with complete measured evidence."""
    blockers: list[str] = []
    if not markhand_soak:
        blockers.append("MARKHAND_SOAK!=1")
        return "not_run", blockers

    if smoke or int(duration_seconds) != int(official_duration):
        blockers.append("smoke_non_qualifying_duration")
    if not prerequisites_ok:
        blockers.append("prerequisites_incomplete")
    if not measured:
        blockers.append("metrics_not_measured")
    if not injection_ok:
        blockers.append("injection_or_recovery_failed")
    if not redaction_ok:
        blockers.append("redaction_failed")

    for name, value in gates.items():
        if value != "pass":
            blockers.append(f"gate:{name}:{value}")

    if blockers:
        # Opt-in but incomplete prerequisites/evidence => incomplete.
        # Hard fail only for measured breaches / redaction / failed recovery after a run.
        hard = any(b.startswith("gate:") and b.endswith(":fail") for b in blockers)
        if redaction_ok is False:
            hard = True
        if measured and not injection_ok:
            hard = True
        return ("fail" if hard else "incomplete"), blockers

    return "pass", []


def build_not_run_report(
    *,
    profile_path: str,
    out_dir: Path,
    git_short: str,
    git_full: str,
    raw_dir: Path,
) -> dict[str, Any]:
    return {
        "issue": ISSUE,
        "generatedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "status": "not_run",
        "markhandSoak": False,
        "smoke": False,
        "smokeNonQualifying": False,
        "profile": profile_path,
        "canonicalReport": CANONICAL,
        "notes": "Stack not opted in; report records workload intent only",
        "blockers": ["MARKHAND_SOAK!=1"],
        "gates": unknown_gates(),
        "metrics": {},
        "prerequisites": {},
        "failureInjection": {},
        "versions": {
            "git": git_short,
            "gitShaFull": git_full,
            "migrationManifestSha256": None,
            "indexSignature": None,
            "imageIds": {},
        },
        "provenance": {
            "gitSha": git_short,
            "gitShaFull": git_full,
            "composeProject": None,
        },
        "redactionScan": {"passed": True, "findings": []},
        "rawDir": str(raw_dir),
        "outDir": str(out_dir),
    }


def _write_text_atomic(path: Path, text: str, *, mode: int = 0o600) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp_name = tempfile.mkstemp(prefix=f".{path.name}.", suffix=".tmp", dir=str(path.parent))
    tmp = Path(tmp_name)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            handle.write(text)
        try:
            os.chmod(tmp, mode)
        except OSError:
            pass
        os.replace(tmp, path)
        try:
            os.chmod(path, mode)
        except OSError:
            pass
    finally:
        if tmp.exists():
            tmp.unlink()


def write_reports(out_dir: Path, payload: dict[str, Any]) -> None:
    """Write o05-soak.json/.md and a thin summary.json pointer (issue=P1B-O05)."""
    out_dir.mkdir(parents=True, exist_ok=True)
    canonical = dict(payload)
    canonical["issue"] = ISSUE
    canonical["canonicalReport"] = CANONICAL
    serialized = json.dumps(canonical, indent=2, sort_keys=True) + "\n"
    findings = redact.scan_text(serialized)
    if findings:
        raise RuntimeError("serialized_report_secret_scan_failed")
    _write_text_atomic(out_dir / CANONICAL, serialized)

    status = canonical.get("status", "not_run")
    notes = canonical.get("notes") or ""
    md = [
        "# P1B-O05 mixed-load soak / qualification",
        "",
        f"- Status: `{status}`",
        f"- Issue: `{ISSUE}`",
        f"- Canonical JSON: `{CANONICAL}`",
        f"- Profile: `{canonical.get('profile')}`",
        f"- Smoke non-qualifying: `{canonical.get('smokeNonQualifying')}`",
        f"- Raw: `{canonical.get('rawDir')}`",
        "",
        "## Notes",
        "",
        str(notes),
        "",
        "## Blockers",
        "",
    ]
    blockers = canonical.get("blockers") or []
    md.extend([f"- `{b}`" for b in blockers] or ["- (none)"])
    md.extend(["", "## Gates", ""])
    for key, value in sorted((canonical.get("gates") or {}).items()):
        md.append(f"- `{key}`: `{value}`")
    md.append("")
    _write_text_atomic(out_dir / "o05-soak.md", "\n".join(md))

    # Backward-compatible summary.json — explicitly O05, never O04.
    summary = {
        "issue": ISSUE,
        "canonicalReport": CANONICAL,
        "generatedAt": canonical.get("generatedAt"),
        "profile": canonical.get("profile"),
        "live": bool(canonical.get("markhandSoak")),
        "status": status,
        "notes": notes,
        "versions": canonical.get("versions") or {},
        "gates": {
            "unboundedGrowth": (canonical.get("gates") or {}).get("unboundedGrowth", "unknown"),
            "recovery": (canonical.get("gates") or {}).get("recovery", "unknown"),
            "postRestoreRetrieval": (canonical.get("gates") or {}).get(
                "postRestoreRetrieval", "unknown"
            ),
        },
        "blockers": blockers,
    }
    _write_text_atomic(out_dir / "summary.json", json.dumps(summary, indent=2, sort_keys=True) + "\n")
    # Keep legacy phase-1b-gate.md in sync with O05 status (honest).
    _write_text_atomic(
        out_dir / "phase-1b-gate.md",
        "# Phase 1B soak / qualification\n\n"
        f"Status: **{status}**\n\n"
        f"{notes}\n\n"
        f"Canonical evidence: `{CANONICAL}` (issue `{ISSUE}`).\n",
    )
