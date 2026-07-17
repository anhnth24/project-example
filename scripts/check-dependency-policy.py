#!/usr/bin/env python3
"""Enforce dependency source, license, lockfile and CI action policy."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
ACTION = re.compile(r"uses:\s*([^@\s]+)@([^\s#]+)")
IMMUTABLE_SHA = re.compile(r"^[0-9a-f]{40}$")


def cargo_policy(root: Path) -> list[str]:
    result = subprocess.run(
        ["cargo", "metadata", "--locked", "--format-version", "1"],
        cwd=root,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode:
        return ["cargo metadata --locked failed"]
    metadata = json.loads(result.stdout)
    errors = []
    for package in metadata["packages"]:
        source = package.get("source")
        if source and source.startswith("git+"):
            errors.append(f"unapproved Cargo git dependency: {package['name']}")
        if source and not package.get("license"):
            errors.append(f"Cargo dependency missing license metadata: {package['name']}")
        manifest = Path(package["manifest_path"]).resolve()
        if source is None and not manifest.is_relative_to(root.resolve()):
            errors.append(f"Cargo path dependency escapes repository: {package['name']}")
    return errors


def static_policy(root: Path) -> list[str]:
    errors: list[str] = []
    lockfiles = sorted(path.relative_to(root).as_posix() for path in root.rglob("pnpm-lock.yaml"))
    if lockfiles != ["pnpm-lock.yaml"]:
        errors.append(f"expected only root pnpm-lock.yaml, found: {lockfiles}")
    lock = (root / "pnpm-lock.yaml").read_text(encoding="utf-8")
    if re.search(r"(?:git\+|github\.com/.+\.git)", lock, re.IGNORECASE):
        errors.append("pnpm lock contains an unapproved git dependency")
    for workflow in (root / ".github/workflows").glob("*.yml"):
        content = workflow.read_text(encoding="utf-8")
        for action, revision in ACTION.findall(content):
            if not IMMUTABLE_SHA.fullmatch(revision):
                errors.append(
                    f"{workflow.relative_to(root)}: action is not SHA-pinned: {action}@{revision}"
                )
    compose = (root / "deploy/dev/compose.yml").read_text(encoding="utf-8")
    if re.search(r"^\s*image:\s*\S+:latest\s*$", compose, re.MULTILINE):
        errors.append("dev Compose image uses mutable latest tag")
    return errors


class DependencyPolicyTests(unittest.TestCase):
    def test_rejects_nested_lock_git_source_mutable_action_and_latest_image(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "pnpm-lock.yaml").write_text("resolution: git+https://example.test/a.git\n")
            nested = root / "app/pnpm-lock.yaml"
            nested.parent.mkdir()
            nested.write_text("lockfileVersion: '9.0'\n")
            workflow = root / ".github/workflows/ci.yml"
            workflow.parent.mkdir(parents=True)
            workflow.write_text("steps:\n  - uses: actions/checkout@v4\n")
            compose = root / "deploy/dev/compose.yml"
            compose.parent.mkdir(parents=True)
            compose.write_text("services:\n  db:\n    image: postgres:latest\n")
            errors = static_policy(root)
            self.assertEqual(len(errors), 4)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(DependencyPolicyTests)
        return 0 if unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful() else 1
    errors = static_policy(ROOT) + cargo_policy(ROOT)
    if errors:
        print("dependency policy failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    print("dependency policy passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
