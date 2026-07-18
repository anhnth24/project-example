#!/usr/bin/env python3
"""Measure the approved Phase 0 architecture-decision gate."""

from __future__ import annotations

import argparse
import json
import sys
import tempfile
import unittest
from pathlib import Path, PurePosixPath


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_MANIFEST = ROOT / "docs/adr/phase0-decisions.json"
REQUIRED_IDS = {
    "document-artifact-model",
    "tenant-isolation-rls",
    "pg-partition-strategy",
    "qdrant-topology",
    "auth-session-lifecycle",
    "model-index-migration",
    "backup-recovery-order",
}


def validate(path: Path) -> tuple[list[str], int]:
    if not path.is_file():
        return ([f"decision manifest missing: {path}"], 0)
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        return ([f"decision manifest unreadable: {error}"], 0)
    errors: list[str] = []
    if payload.get("version") != 1 or not isinstance(payload.get("decisions"), list):
        return (["decision manifest requires version=1 and decisions array"], 0)
    seen: set[str] = set()
    evidence_seen: set[str] = set()
    approved = 0
    for decision in payload["decisions"]:
        decision_id = decision.get("id")
        if decision_id in seen:
            errors.append(f"duplicate decision id: {decision_id}")
        seen.add(decision_id)
        if decision_id not in REQUIRED_IDS:
            errors.append(f"unknown decision id: {decision_id}")
        if decision.get("status") == "accepted":
            approved += 1
        else:
            errors.append(f"decision not accepted: {decision_id}")
        evidence = decision.get("evidence")
        if not isinstance(evidence, str) or not evidence.strip():
            errors.append(f"decision missing evidence: {decision_id}")
        else:
            pure = PurePosixPath(evidence)
            evidence_path = (ROOT / pure).resolve()
            if (
                pure.is_absolute()
                or ".." in pure.parts
                or pure.as_posix() != evidence
                or not evidence.startswith("docs/adr/")
                or pure.suffix != ".md"
                or not evidence_path.is_relative_to(ROOT.resolve())
            ):
                errors.append(f"decision evidence path is invalid: {evidence}")
            elif evidence in evidence_seen:
                errors.append(f"decision evidence reused: {evidence}")
            elif not evidence_path.is_file():
                errors.append(f"decision evidence missing: {evidence}")
            else:
                content = evidence_path.read_text(encoding="utf-8")
                if "- Status: Accepted" not in content or str(decision_id) not in content:
                    errors.append(f"decision evidence is not accepted or keyed: {evidence}")
            evidence_seen.add(evidence)
    missing = REQUIRED_IDS - seen
    if missing:
        errors.append(f"missing decisions: {sorted(missing)}")
    return errors, approved


class DecisionGateTests(unittest.TestCase):
    def test_rejects_missing_duplicate_and_unaccepted_decisions(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "decisions.json"
            path.write_text(
                json.dumps(
                    {
                        "version": 1,
                        "decisions": [
                            {
                                "id": "document-artifact-model",
                                "status": "proposed",
                                "evidence": "missing.md",
                            },
                            {
                                "id": "document-artifact-model",
                                "status": "accepted",
                                "evidence": "missing.md",
                            },
                        ],
                    }
                )
            )
            errors, approved = validate(path)
            self.assertEqual(approved, 1)
            self.assertTrue(any("duplicate" in error for error in errors))
            self.assertTrue(any("not accepted" in error for error in errors))
            self.assertTrue(any("evidence path is invalid" in error for error in errors))
            self.assertTrue(any("missing decisions" in error for error in errors))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(DecisionGateTests)
        return 0 if unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful() else 1
    errors, approved = validate(args.manifest)
    if errors:
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    print(json.dumps({"metric": "approved_architecture_decisions", "value": approved}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
