"""Evidence report builder (redacted, schema-aligned).

Hermetic/tracked reports are deterministic (no timestamp, random run id, branch, or HEAD).
Live runtime evidence writes a separate gitignored artifact with runtime identity.
"""

from __future__ import annotations

import json
import subprocess
import uuid
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from .redaction import assert_no_forbidden_evidence, redact_value

SEVERITY_RANK = {"none": 0, "low": 1, "medium": 2, "high": 3, "critical": 4}

# Stable identity for committed hermetic evidence (repo-relative, deterministic).
HERMETIC_GENERATED_AT = "1970-01-01T00:00:00Z"
HERMETIC_RUN_ID = "00000000-0000-4000-8000-000000000004"
HERMETIC_GIT = {"branch": "hermetic", "commit": "0" * 40}

TRACKED_JSON = Path("bench/markhand_web/reports/p1b-o04-vertical-slice.json")
TRACKED_MD = Path("bench/markhand_web/reports/p1b-o04-vertical-slice.md")
LIVE_JSON = Path("bench/markhand_web/reports/p1b-o04-vertical-slice.live.json")
LIVE_MD = Path("bench/markhand_web/reports/p1b-o04-vertical-slice.live.md")


@dataclass
class CaseResult:
    id: str
    matrix: str
    status: str
    http_statuses: list[int] = field(default_factory=list)
    postconditions: dict[str, bool] = field(default_factory=dict)
    severity: str = "none"
    blocker_code: str | None = None
    opaque_refs: dict[str, str] = field(default_factory=dict)
    notes: str = ""

    def to_json(self) -> dict[str, Any]:
        payload = {
            "id": self.id,
            "matrix": self.matrix,
            "status": self.status,
            "httpStatuses": list(self.http_statuses),
            "postconditions": dict(self.postconditions),
            "severity": self.severity,
            "blockerCode": self.blocker_code,
            "opaqueRefs": dict(self.opaque_refs),
            "notes": self.notes,
        }
        return redact_value(payload)


def runtime_git_meta(root: Path) -> dict[str, str]:
    def _run(args: list[str]) -> str:
        try:
            return subprocess.check_output(args, cwd=root, text=True).strip()
        except (OSError, subprocess.CalledProcessError):
            return "unknown"

    return {
        "branch": _run(["git", "branch", "--show-current"]),
        "commit": _run(["git", "rev-parse", "HEAD"]),
    }


def _summary(cases: list[CaseResult]) -> dict[str, int]:
    return {
        "passed": sum(1 for c in cases if c.status == "pass"),
        "failed": sum(1 for c in cases if c.status == "fail"),
        "blocked": sum(1 for c in cases if c.status == "blocked"),
        "skippedOptional": sum(1 for c in cases if c.status == "optional_unavailable"),
        "highCritical": sum(
            1 for c in cases if SEVERITY_RANK.get(c.severity, 0) >= SEVERITY_RANK["high"]
        ),
    }


def _max_severity(cases: list[CaseResult], summary: dict[str, int]) -> str:
    max_sev = "none"
    for case in cases:
        if SEVERITY_RANK.get(case.severity, 0) > SEVERITY_RANK[max_sev]:
            max_sev = case.severity
    if summary["highCritical"] > 0 and SEVERITY_RANK[max_sev] < SEVERITY_RANK["high"]:
        max_sev = "high"
    return max_sev


def build_report(
    *,
    root: Path,
    mode: str,
    cases: list[CaseResult],
    blockers: list[str],
    claims_live: bool,
    run_id: str | None = None,
) -> dict[str, Any]:
    """Build an evidence report.

    Hermetic mode is deterministic. Live mode embeds runtime identity for the
    gitignored live artifact only.
    """
    summary = _summary(cases)
    max_sev = _max_severity(cases, summary)

    if mode == "hermetic":
        generated_at = HERMETIC_GENERATED_AT
        resolved_run_id = HERMETIC_RUN_ID
        git = dict(HERMETIC_GIT)
    else:
        generated_at = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
        resolved_run_id = run_id or str(uuid.uuid4())
        git = runtime_git_meta(root)

    report = {
        "version": 1,
        "issue": "P1B-O04",
        "generatedAt": generated_at,
        "runId": resolved_run_id,
        "mode": mode,
        "git": git,
        "claimsLiveVerticalSlice": claims_live,
        "summary": summary,
        "cases": [case.to_json() for case in cases],
        "blockers": [str(b) for b in blockers],
        "severity": max_sev,
    }
    text = json.dumps(report, indent=2, sort_keys=True)
    leaks = assert_no_forbidden_evidence(text)
    if leaks:
        raise RuntimeError("evidence redaction failed: " + "; ".join(leaks))
    return report


def write_markdown_report(report: dict[str, Any], path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    s = report["summary"]
    lines = [
        "# P1B-O04 evidence — vertical-slice / security release suite",
        "",
        f"Status mode: `{report['mode']}` · `claimsLiveVerticalSlice`: "
        f"**{str(report['claimsLiveVerticalSlice']).lower()}**",
        "",
    ]
    if report["mode"] == "hermetic":
        lines.extend(
            [
                "Identity: hermetic deterministic (no runtime timestamp / branch / HEAD).",
                f"Run id (stable): `{report['runId']}`",
                "",
            ]
        )
    else:
        lines.extend(
            [
                f"Run id (runtime): `{report['runId']}`",
                f"Git: `{report['git']['branch']}` @ `{report['git']['commit'][:12]}`",
                f"Generated at: `{report['generatedAt']}`",
                "",
            ]
        )
    lines.extend(
        [
            f"Severity: `{report['severity']}`",
            "",
            "## Summary",
            "",
            f"- passed: {s['passed']}",
            f"- failed: {s['failed']}",
            f"- blocked: {s['blocked']}",
            f"- optional unavailable: {s['skippedOptional']}",
            f"- high/critical cases: {s['highCritical']}",
            "",
            "## Blockers",
            "",
        ]
    )
    if report["blockers"]:
        for item in report["blockers"]:
            lines.append(f"- {item}")
    else:
        lines.append("- (none)")
    lines.extend(["", "## Cases", ""])
    for case in report["cases"]:
        blocker = case.get("blockerCode") or "none"
        lines.append(
            f"- `{case['id']}` [{case['matrix']}] → **{case['status']}** "
            f"(severity={case.get('severity', 'none')}; blocker={blocker}; "
            f"http={case.get('httpStatuses')})"
        )
        if case.get("notes"):
            lines.append(f"  - {case['notes']}")
    lines.extend(
        [
            "",
            "## Non-claims",
            "",
            "- Hermetic mode validates harness/fixtures/gates only.",
            "- This report never embeds document text, prompts, tokens, passwords,",
            "  signed URLs, raw object keys, or tenant IDs.",
            "- Live vertical-slice pass requires Docker POC stack + confirm gates",
            "  **and** production upload→documentId/versionId/jobId wiring.",
            "- Runtime live evidence is written to a gitignored `.live` artifact;",
            "  the tracked hermetic report stays deterministic.",
            "",
        ]
    )
    path.write_text("\n".join(lines), encoding="utf-8")


def write_json_report(report: dict[str, Any], path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def write_tracked_hermetic(root: Path, report: dict[str, Any]) -> None:
    write_json_report(report, root / TRACKED_JSON)
    write_markdown_report(report, root / TRACKED_MD)


def write_live_runtime(root: Path, report: dict[str, Any]) -> None:
    write_json_report(report, root / LIVE_JSON)
    write_markdown_report(report, root / LIVE_MD)
