#!/usr/bin/env python3
"""Classify changed paths for path-aware GitHub CI jobs."""

from __future__ import annotations

import argparse
import fnmatch
import subprocess
import unittest
from pathlib import Path


SHARED = (
    ".github/workflows/ci.yml",
    "Makefile",
    "scripts/classify-ci-changes.py",
)
GROUPS = {
    "rust": SHARED
    + (
        "Cargo.lock",
        "Cargo.toml",
        "**/Cargo.toml",
        "crates/**",
        "app/src-tauri/**",
        "rustfmt.toml",
        "clippy.toml",
        "scripts/check-rust*",
        "scripts/check-knowledge*",
        "scripts/check-architecture-boundaries.py",
    ),
    "knowledge": SHARED
    + (
        "Cargo.lock",
        "Cargo.toml",
        "crates/knowledge/**",
        "crates/server/tests/knowledge_consumer.rs",
        "app/src-tauri/src/knowledge.rs",
        "app/src-tauri/src/knowledge_contract.rs",
        "app/src-tauri/fixtures/knowledge/**",
        "app/src/lib/ipc.ts",
        "app/src/lib/types.ts",
        "app/src/lib/knowledgeContract.test.ts",
        "scripts/check-knowledge*",
        "docs/runbooks/knowledge-index-compatibility.md",
    ),
    "frontend": SHARED
    + (
        "package.json",
        "pnpm-lock.yaml",
        "pnpm-workspace.yaml",
        "app/package.json",
        "app/tsconfig.json",
        "app/vite.config.ts",
        "app/eslint.config.js",
        "app/src/**",
    ),
    "web": SHARED
    + (
        "package.json",
        "pnpm-lock.yaml",
        "pnpm-workspace.yaml",
        "web/**",
        "crates/server/openapi/**",
    ),
    "dev_stack": SHARED
    + (
        "deploy/dev/**",
        "deploy/scripts/**",
        "docs/runbooks/local-development.md",
    ),
}


def classify(paths: list[str]) -> dict[str, bool]:
    return {
        name: any(
            fnmatch.fnmatch(path, pattern)
            for path in paths
            for pattern in patterns
        )
        for name, patterns in GROUPS.items()
    }


def changed_paths(base: str, head: str) -> list[str]:
    if not base or set(base) == {"0"}:
        base = subprocess.check_output(
            ["git", "rev-parse", f"{head}^"], text=True
        ).strip()
    return subprocess.check_output(
        ["git", "diff", "--name-only", base, head], text=True
    ).splitlines()


class ClassifierTests(unittest.TestCase):
    def test_docs_only_uses_static_job(self) -> None:
        self.assertFalse(any(classify(["docs/notes.md"]).values()))

    def test_knowledge_adapter_activates_rust_knowledge_and_frontend(self) -> None:
        result = classify(["app/src/lib/knowledgeContract.test.ts"])
        self.assertTrue(result["knowledge"])
        self.assertTrue(result["frontend"])
        self.assertFalse(result["web"])
        self.assertFalse(result["dev_stack"])

    def test_deploy_change_activates_only_dev_stack(self) -> None:
        result = classify(["deploy/dev/compose.yml"])
        self.assertTrue(result["dev_stack"])
        self.assertFalse(result["rust"])
        self.assertFalse(result["frontend"])

    def test_ci_or_makefile_change_activates_every_group(self) -> None:
        self.assertTrue(all(classify([".github/workflows/ci.yml"]).values()))
        self.assertTrue(all(classify(["Makefile"]).values()))
        self.assertTrue(all(classify(["scripts/classify-ci-changes.py"]).values()))

    def test_root_lockfile_activates_both_frontends(self) -> None:
        result = classify(["pnpm-lock.yaml"])
        self.assertTrue(result["frontend"])
        self.assertTrue(result["web"])


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base")
    parser.add_argument("--head", default="HEAD")
    parser.add_argument("--github-output", type=Path)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(ClassifierTests)
        return 0 if unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful() else 1
    if not args.base:
        parser.error("--base is required unless --self-test is used")

    paths = changed_paths(args.base, args.head)
    result = classify(paths)
    if args.github_output:
        with args.github_output.open("a", encoding="utf-8") as output:
            for name, enabled in result.items():
                print(f"{name}={str(enabled).lower()}", file=output)
    for name, enabled in result.items():
        print(f"{name}={str(enabled).lower()}")
    if paths:
        print("Changed files:", *(f"- {path}" for path in paths), sep="\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
