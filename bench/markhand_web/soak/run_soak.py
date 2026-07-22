#!/usr/bin/env python3
"""Mixed-load soak driver for Markhand Web Phase 1B (P1B-O05).

Never writes a green `pass` without real numeric gate evidence. Opting in with
`MARKHAND_SOAK=1` alone yields `incomplete`, not success.
"""

from __future__ import annotations

import argparse
import json
import os
import time
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--profile", required=True)
    parser.add_argument("--out", required=True)
    args = parser.parse_args()
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    profile = Path(args.profile).read_text(encoding="utf-8")
    live = os.environ.get("MARKHAND_SOAK") == "1"
    evidence = os.environ.get("MARKHAND_SOAK_EVIDENCE") == "1"
    if not live:
        status = "not_run"
        notes = "Stack not opted in; report records workload intent only"
    elif not evidence:
        status = "incomplete"
        notes = (
            "MARKHAND_SOAK=1 set but MARKHAND_SOAK_EVIDENCE=1 missing; "
            "numeric gates not filled — not a pass"
        )
    else:
        status = "incomplete"
        notes = (
            "Evidence mode enabled; operator must replace gate values with measured "
            "results before claiming Done"
        )
    summary = {
        "generatedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "profile": args.profile,
        "live": live,
        "status": status,
        "notes": notes,
        "profilePreview": profile.splitlines()[:20],
        "versions": {
            "git": os.popen("git rev-parse --short HEAD").read().strip() or "unknown",
        },
        "gates": {
            "unboundedGrowth": "unknown",
            "recovery": "unknown",
            "postRestoreRetrieval": "unknown",
        },
    }
    # Refuse to emit pass unless every gate is an explicit pass string.
    if all(value == "pass" for value in summary["gates"].values()):
        summary["status"] = "pass"
    (out / "summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    (out / "phase-1b-gate.md").write_text(
        "# Phase 1B soak / qualification\n\n"
        f"Status: **{summary['status']}**\n\n"
        f"{notes}\n\n"
        "Numeric gates must be filled from a live mixed-load run before O05 Done.\n",
        encoding="utf-8",
    )
    print(out / "summary.json")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
