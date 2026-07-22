"""Evidence report builder (redacted, schema-aligned)."""

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


def git_meta(root: Path) -> dict[str, str]:
    def _run(args: list[str]) -> str:
        try:
            return subprocess.check_output(args, cwd=root, text=True).strip()
        except (OSError, subprocess.CalledProcessError):
            return "unknown"

    return {
        "branch": _run(["git", "branch", "--show-current"]),
        "commit": _run(["git", "rev-parse", "HEAD"]),
    }


def build_report(
    *,
    root: Path,
    mode: str,
    cases: list[CaseResult],
    blockers: list[str],
    claims_live: bool,
    run_id: str | None = None,
) -> dict[str, Any]:
    summary = {
        "passed": sum(1 for c in cases if c.status == "pass"),
        "failed": sum(1 for c in cases if c.status == "fail"),
        "blocked": sum(1 for c in cases if c.status == "blocked"),
        "skippedOptional": sum(1 for c in cases if c.status == "optional_unavailable"),
        "highCritical": sum(
            1 for c in cases if SEVERITY_RANK.get(c.severity, 0) >= SEVERITY_RANK["high"]
        ),
    }
    max_sev = "none"
    for case in cases:
        if SEVERITY_RANK.get(case.severity, 0) > SEVERITY_RANK[max_sev]:
            max_sev = case.severity
    if summary["highCritical"] > 0 and SEVERITY_RANK[max_sev] < SEVERITY_RANK["high"]:
        max_sev = "high"

    report = {
        "version": 1,
        "issue": "P1B-O04",
        "generatedAt": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "runId": run_id or str(uuid.uuid4()),
        "mode": mode,
        "git": git_meta(root),
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
        f"Run id (opaque): `{report['runId']}`",
        f"Git: `{report['git']['branch']}` @ `{report['git']['commit'][:12]}`",
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
            "",
        ]
    )
    path.write_text("\n".join(lines), encoding="utf-8")


def write_json_report(report: dict[str, Any], path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
