#!/usr/bin/env python3
"""Hermetic entry — never claims live vertical slice."""

from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from harness.runner import run_hermetic_blocked_report  # noqa: E402


def main() -> int:
    report = run_hermetic_blocked_report()
    print(
        f"P1B-O04 hermetic evidence written "
        f"(claimsLiveVerticalSlice={report['claimsLiveVerticalSlice']}, "
        f"blocked={report['summary']['blocked']})"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())