#!/usr/bin/env python3
"""Hermetic contract tests for the deployed-1c-integration CI job wiring."""

from __future__ import annotations

import argparse
import re
import sys
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
CI_WORKFLOW = REPO_ROOT / ".github/workflows/ci.yml"
JOB_NAME = "deployed-1c-integration"


def extract_job_block(workflow_text: str, job_name: str) -> str:
    pattern = rf"^  {re.escape(job_name)}:\n"
    match = re.search(pattern, workflow_text, flags=re.MULTILINE)
    if match is None:
        raise ValueError(f"job {job_name!r} not found")
    start = match.start()
    tail = workflow_text[start + 1 :]
    next_job = re.search(r"^  [A-Za-z0-9_-]+:\n", tail, flags=re.MULTILINE)
    end = start + 1 + (next_job.start() if next_job else len(tail))
    return workflow_text[start:end]


def deployed_job_contract_errors(job_block: str) -> list[str]:
    errors: list[str] = []

    if "run-phase1c-denial-suite.py" not in job_block:
        errors.append("must invoke scripts/run-phase1c-denial-suite.py")

    if "MARKHAND_TEST_REQUIRED" not in job_block:
        errors.append("must set MARKHAND_TEST_REQUIRED")
    elif re.search(r'MARKHAND_TEST_REQUIRED:\s*"1"', job_block) is None:
        errors.append('MARKHAND_TEST_REQUIRED must be "1"')

    if re.search(
        r"cargo test -p fileconv-server --test '\*'",
        job_block,
    ) or re.search(r'cargo test -p fileconv-server --test "\*"', job_block):
        errors.append("must not run wildcard cargo test --test '*'")

    if "test-output.log" in job_block:
        errors.append("must not upload raw cargo test-output.log")

    if "render-phase1c-denial-report.py" not in job_block:
        errors.append("must invoke scripts/render-phase1c-denial-report.py")

    if "manifest-run.json" not in job_block:
        errors.append("artifact must include manifest-run.json")

    if "phase1c-denial-report.md" not in job_block:
        errors.append("artifact must include phase1c-denial-report.md")

    teardown_steps = re.findall(
        r"- name: Tear down POC stack\n(?:.*?\n)*?        if: always\(\)",
        job_block,
        flags=re.DOTALL,
    )
    if not teardown_steps:
        errors.append("Tear down POC stack step must use if: always()")

    upload_steps = re.findall(
        r"- name: Upload 1C integration report\n(?:.*?\n)*?        if: always\(\)",
        job_block,
        flags=re.DOTALL,
    )
    if not upload_steps:
        errors.append("Upload 1C integration report step must use if: always()")

    render_steps = re.findall(
        r"- name: Render Phase 1C denial report\n(?:.*?\n)*?        if: always\(\)",
        job_block,
        flags=re.DOTALL,
    )
    if not render_steps:
        errors.append("Render Phase 1C denial report step must use if: always()")

    if "Enforce 1C-12 deployed gate" not in job_block:
        errors.append("must include Enforce 1C-12 deployed gate step")

    return errors


class Deployed1cWorkflowContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow_text = CI_WORKFLOW.read_text(encoding="utf-8")
        cls.job_block = extract_job_block(cls.workflow_text, JOB_NAME)

    def test_deployed_job_uses_canonical_runner_and_preserves_always_steps(self) -> None:
        errors = deployed_job_contract_errors(self.job_block)
        self.assertEqual(errors, [], "\n".join(errors))


def run_self_tests() -> int:
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(Deployed1cWorkflowContractTests)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if not args.self_test:
        parser.error("--self-test is required")
    return run_self_tests()


if __name__ == "__main__":
    raise SystemExit(main())
