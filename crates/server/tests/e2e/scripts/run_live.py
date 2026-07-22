#!/usr/bin/env python3
"""Live E2E entry — fails hard without confirm gates / stack / prerequisites."""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from harness.runner import run_live  # noqa: E402


def main() -> int:
    try:
        report = run_live()
    except Exception as exc:  # noqa: BLE001 — surface actionable live failures
        print(f"P1B-O04 live suite FAILED: {exc}", file=sys.stderr)
        return 1
    print(
        f"P1B-O04 live suite OK "
        f"(passed={report['summary']['passed']}, "
        f"claimsLiveVerticalSlice={report['claimsLiveVerticalSlice']})"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())