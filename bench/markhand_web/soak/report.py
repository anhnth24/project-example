"""O05 report build / evaluate / write (canonical o05-soak.json)."""

from __future__ import annotations

import json
import time
from pathlib import Path
from typing import Any


ISSUE = "P1B-O05"
CANONICAL = "o05-soak.json"


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
    }


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


def write_reports(out_dir: Path, payload: dict[str, Any]) -> None:
    """Write o05-soak.json/.md and a thin summary.json pointer (issue=P1B-O05)."""
    out_dir.mkdir(parents=True, exist_ok=True)
    canonical = dict(payload)
    canonical["issue"] = ISSUE
    canonical["canonicalReport"] = CANONICAL
    (out_dir / CANONICAL).write_text(
        json.dumps(canonical, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )

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
    (out_dir / "o05-soak.md").write_text("\n".join(md), encoding="utf-8")

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
    (out_dir / "summary.json").write_text(
        json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    # Keep legacy phase-1b-gate.md in sync with O05 status (honest).
    (out_dir / "phase-1b-gate.md").write_text(
        "# Phase 1B soak / qualification\n\n"
        f"Status: **{status}**\n\n"
        f"{notes}\n\n"
        f"Canonical evidence: `{CANONICAL}` (issue `{ISSUE}`).\n",
        encoding="utf-8",
    )
