#!/usr/bin/env python3
"""Fail if backlog catalog Status drifts from github-issues.json metadata.

Compares each issue id's normalized status from phase README catalogs against
the top-level status field and the body Status line in
plans/markhand-web/backlog/github-issues.json.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))

import importlib.util

_spec = importlib.util.spec_from_file_location(
    "build_roadmap", ROOT / "scripts/build-roadmap.py"
)
roadmap = importlib.util.module_from_spec(_spec)
assert _spec.loader is not None
sys.modules["build_roadmap"] = roadmap
_spec.loader.exec_module(roadmap)

_spec2 = importlib.util.spec_from_file_location(
    "sync_github_issues", ROOT / "scripts/sync-github-issues.py"
)
sync = importlib.util.module_from_spec(_spec2)
assert _spec2.loader is not None
sys.modules["sync_github_issues"] = sync
_spec2.loader.exec_module(sync)

BODY_STATUS = re.compile(r"- Status:\s*`(?P<status>[a-z_]+)`", re.IGNORECASE)
JSON_PATH = ROOT / "plans/markhand-web/backlog/github-issues.json"


def main() -> int:
    catalog = {issue.issue_id: issue.status for issue in sync.load_catalog_issues()}
    payload = json.loads(JSON_PATH.read_text(encoding="utf-8"))
    issues = payload.get("issues") or []
    errors: list[str] = []
    seen: set[str] = set()
    for item in issues:
        issue_id = item.get("id")
        if not issue_id:
            errors.append("github-issues entry missing id")
            continue
        seen.add(issue_id)
        top = item.get("status")
        body = item.get("body") or ""
        body_match = BODY_STATUS.search(body)
        body_status = body_match.group("status") if body_match else None
        catalog_status = catalog.get(issue_id)
        if catalog_status is None:
            errors.append(f"{issue_id}: present in github-issues.json but missing from catalogs")
            continue
        if top != catalog_status:
            errors.append(
                f"{issue_id}: github-issues.json status={top!r} != catalog {catalog_status!r}"
            )
        if body_status != catalog_status:
            errors.append(
                f"{issue_id}: github-issues.json body Status={body_status!r} != catalog {catalog_status!r}"
            )
    for issue_id, status in catalog.items():
        if issue_id not in seen:
            errors.append(f"{issue_id}: in catalog ({status}) but missing from github-issues.json")

    # Hard pin: O01 must remain in_progress until full evidence rebuild passes.
    o01 = catalog.get("P1B-O01")
    if o01 != "in_progress":
        errors.append(f"P1B-O01 catalog status must be in_progress (got {o01!r})")

    if errors:
        print("github issue consistency FAILED:", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        return 1
    print(f"github issue consistency OK ({len(catalog)} issues)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
