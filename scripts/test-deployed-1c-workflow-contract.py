#!/usr/bin/env python3
"""Hermetic structural contract tests for deployed-1c-integration CI wiring."""

from __future__ import annotations

import argparse
import re
import sys
import unittest
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence

REPO_ROOT = Path(__file__).resolve().parents[1]
CI_WORKFLOW = REPO_ROOT / ".github/workflows/ci.yml"
JOB_NAME = "deployed-1c-integration"

REQUIRED_STEP_ORDER = (
    "Boot POC Compose stack",
    "Run Phase 1C denial suite (deployed POC stack)",
    "Tear down POC stack",
    "Render Phase 1C denial report",
    "Upload 1C integration report",
    "Enforce 1C-12 deployed gate",
)

RENDER_COMMAND_PREFIX = "python3 scripts/render-phase1c-denial-report.py"
TEARDOWN_COMMAND = "docker compose -f deploy/compose.poc.yml down -v"
ARTIFACT_PATHS = (
    "${{ runner.temp }}/markhand-1c-integration/manifest-run.json",
    "${{ runner.temp }}/markhand-1c-integration/phase1c-denial-report.md",
)


@dataclass(frozen=True)
class WorkflowStep:
    name: str
    run: str | None
    if_condition: str | None
    step_id: str | None
    uses: str | None


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


def parse_job_steps(job_block: str) -> list[WorkflowStep]:
    steps: list[WorkflowStep] = []
    chunks = re.split(r"\n      - ", job_block)
    for chunk in chunks[1:]:
        name_match = re.match(r"name: (.+)\n", chunk)
        if name_match is None:
            continue
        name = name_match.group(1).strip()
        step_id = None
        id_match = re.search(r"^        id: (.+)\n", chunk, flags=re.MULTILINE)
        if id_match:
            step_id = id_match.group(1).strip()
        if_condition = None
        if_match = re.search(r"^        if: (.+)\n", chunk, flags=re.MULTILINE)
        if if_match:
            if_condition = if_match.group(1).strip()
        uses = None
        uses_match = re.search(r"^        uses: (.+)\n", chunk, flags=re.MULTILINE)
        if uses_match:
            uses = uses_match.group(1).strip()
        run = None
        run_match = re.search(r"^        run: \|\n((?:          .*\n?)*)", chunk, flags=re.MULTILINE)
        if run_match:
            run = "\n".join(line[10:] for line in run_match.group(1).splitlines())
        steps.append(
            WorkflowStep(
                name=name,
                run=run,
                if_condition=if_condition,
                step_id=step_id,
                uses=uses,
            )
        )
    return steps


def step_index(steps: Sequence[WorkflowStep], name: str) -> int:
    for index, step in enumerate(steps):
        if step.name == name:
            return index
    raise ValueError(f"step {name!r} not found")


def deployed_job_contract_errors(job_block: str) -> list[str]:
    errors: list[str] = []
    steps = parse_job_steps(job_block)
    names = [step.name for step in steps]

    if 'MARKHAND_TEST_REQUIRED: "1"' not in job_block:
        errors.append('job env must set MARKHAND_TEST_REQUIRED: "1"')

    if re.search(r"cargo test -p fileconv-server --test '\*'", job_block):
        errors.append("must not run wildcard cargo test --test '*'")

    if "test-output.log" in job_block:
        errors.append("must not upload raw cargo test-output.log")

    if "PHASE1C_MANIFEST_SHA256" not in job_block:
        errors.append("must resolve PHASE1C_MANIFEST_SHA256 before render")

    try:
        ordered = [step_index(steps, name) for name in REQUIRED_STEP_ORDER]
    except ValueError as exc:
        errors.append(str(exc))
        return errors

    if ordered != sorted(ordered):
        errors.append(
            "required steps must appear in order: "
            + " -> ".join(REQUIRED_STEP_ORDER)
        )

    if names[-1] != "Enforce 1C-12 deployed gate":
        errors.append("Enforce 1C-12 deployed gate must be the final step")

    runner = steps[step_index(steps, "Run Phase 1C denial suite (deployed POC stack)")]
    if runner.step_id != "phase1c_denial_suite":
        errors.append("runner step must have id phase1c_denial_suite")
    if runner.run is None:
        errors.append("runner step must have a run block")
    else:
        if not re.search(
            r"(?m)^[ \t]*python3 scripts/run-phase1c-denial-suite\.py \\",
            runner.run,
        ):
            errors.append("runner step must invoke canonical denial suite command")
        for token in (
            "--manifest crates/server/tests/fixtures/multi-org-denial.manifest.json",
            '--output "$MARKHAND_1C_OUTPUT_DIR/manifest-run.json"',
        ):
            if token not in runner.run:
                errors.append(f"runner step must include {token!r}")
    if "runner_exit_code=" not in (runner.run or ""):
        errors.append("runner step must capture runner_exit_code output")

    teardown = steps[step_index(steps, "Tear down POC stack")]
    if teardown.if_condition != "always()":
        errors.append("teardown step must use if: always()")
    if teardown.step_id != "phase1c_teardown":
        errors.append("teardown step must have id phase1c_teardown")
    if teardown.run is None or TEARDOWN_COMMAND not in teardown.run:
        errors.append("teardown step must run docker compose down -v")
    if "teardown_exit_code=" not in (teardown.run or ""):
        errors.append("teardown step must capture teardown_exit_code output")

    render = steps[step_index(steps, "Render Phase 1C denial report")]
    if render.if_condition != "always()":
        errors.append("render step must use if: always()")
    if render.step_id != "phase1c_render":
        errors.append("render step must have id phase1c_render")
    if render.run is None or RENDER_COMMAND_PREFIX not in render.run:
        errors.append("render step must invoke render-phase1c-denial-report.py")
    if "--expected-git-sha" not in (render.run or ""):
        errors.append("render step must pass trusted expected git sha")
    if "--expected-manifest-sha256" not in (render.run or ""):
        errors.append("render step must pass trusted expected manifest digest")
    if "--teardown-exit-code" not in (render.run or ""):
        errors.append("render step must pass teardown exit code")
    if "|| true" in (render.run or ""):
        errors.append("render step must not hide failures with || true")
    if "render_exit_code=" not in (render.run or ""):
        errors.append("render step must capture render_exit_code output")

    upload = steps[step_index(steps, "Upload 1C integration report")]
    if upload.if_condition != "always()":
        errors.append("upload step must use if: always()")
    if upload.uses is None or "actions/upload-artifact" not in upload.uses:
        errors.append("upload step must use actions/upload-artifact")
    upload_chunk = job_block.split("- name: Upload 1C integration report", 1)[-1]
    if "if-no-files-found: error" not in upload_chunk:
        errors.append("upload step must set if-no-files-found: error")
    for artifact_path in ARTIFACT_PATHS:
        if artifact_path not in upload_chunk:
            errors.append(f"upload step must include exact artifact path {artifact_path}")
    if "markhand-1c-integration/**" in upload_chunk:
        errors.append("upload step must not use wildcard artifact glob")

    enforce = steps[step_index(steps, "Enforce 1C-12 deployed gate")]
    if enforce.if_condition != "always()":
        errors.append("enforce step must use if: always()")
    if enforce.run is None:
        errors.append("enforce step must have a run block")
    else:
        for token in (
            "runner_exit_code",
            "teardown_exit_code",
            "render_exit_code",
            "manifest-run.json",
            "phase1c-denial-report.md",
        ):
            if token not in enforce.run:
                errors.append(f"enforce step must check {token}")

    render_index = step_index(steps, "Render Phase 1C denial report")
    teardown_index = step_index(steps, "Tear down POC stack")
    if render_index <= teardown_index:
        errors.append("render must occur after teardown")

    return errors


class Deployed1cWorkflowContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow_text = CI_WORKFLOW.read_text(encoding="utf-8")
        cls.job_block = extract_job_block(cls.workflow_text, JOB_NAME)

    def test_deployed_job_structural_contract(self) -> None:
        errors = deployed_job_contract_errors(self.job_block)
        self.assertEqual(errors, [], "\n".join(errors))

    def test_comment_mention_cannot_satisfy_runner_contract(self) -> None:
        mutated = self.job_block.replace(
            "python3 scripts/run-phase1c-denial-suite.py",
            "echo runner skipped # python3 scripts/run-phase1c-denial-suite.py",
            1,
        )
        errors = deployed_job_contract_errors(mutated)
        self.assertTrue(
            any("canonical denial suite command" in error for error in errors),
            errors,
        )

    def test_unrelated_upload_glob_fails_contract(self) -> None:
        mutated = self.job_block.replace(
            "path: |\n            ${{ runner.temp }}/markhand-1c-integration/manifest-run.json\n"
            "            ${{ runner.temp }}/markhand-1c-integration/phase1c-denial-report.md",
            "path: ${{ runner.temp }}/markhand-1c-integration/**",
            1,
        )
        errors = deployed_job_contract_errors(mutated)
        self.assertTrue(any("wildcard artifact glob" in error for error in errors))


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
