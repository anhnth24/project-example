#!/usr/bin/env python3
"""Contract tests for the Phase 1C deployed gate report template CLI."""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
TEMPLATE = (
    REPO_ROOT / "plans/reports/gate-run-260803-0000-markhand-web-phase1c-denial-suite-report.md"
)
RENDERER = REPO_ROOT / "scripts/render-phase1c-denial-report.py"

REQUIRED_RENDER_FLAGS = (
    "--input",
    "--output",
    "--expected-git-sha",
    "--expected-manifest-sha256",
    "--expected-git-ref",
    "--ci-run-url",
    "--runner-exit-code",
    "--teardown-exit-code",
)


def extract_render_command(template_text: str) -> str:
    match = re.search(
        r"python3 scripts/render-phase1c-denial-report\.py \\\n((?:  .+\n)+)",
        template_text,
    )
    if match is None:
        raise ValueError("render command block missing from template")
    return "python3 scripts/render-phase1c-denial-report.py \\\n" + match.group(1)


def renderer_help_flags() -> set[str]:
    completed = subprocess.run(
        [sys.executable, str(RENDERER), "--help"],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        check=True,
    )
    return set(re.findall(r"(--[\w-]+)", completed.stdout))


class Phase1cTemplateContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.template_text = TEMPLATE.read_text(encoding="utf-8")
        cls.render_command = extract_render_command(cls.template_text)
        cls.renderer_flags = renderer_help_flags()

    def test_template_render_command_uses_current_trusted_cli(self) -> None:
        for flag in REQUIRED_RENDER_FLAGS:
            self.assertIn(flag, self.render_command, f"missing {flag} in template")
            self.assertIn(flag, self.renderer_flags, f"renderer no longer exposes {flag}")
        self.assertNotIn("--gate", self.render_command)
        self.assertNotIn("--environment-id", self.render_command)
        self.assertNotIn("--gate-1c13-status", self.render_command)

    def test_template_reproduction_render_command_is_executable(self) -> None:
        completed = subprocess.run(
            [
                sys.executable,
                str(RENDERER),
                "--self-test",
            ],
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(0, completed.returncode, completed.stdout + completed.stderr)


def run_self_tests() -> int:
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(Phase1cTemplateContractTests)
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
