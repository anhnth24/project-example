#!/usr/bin/env python3
"""Classify changed paths for path-aware GitHub CI jobs."""

from __future__ import annotations

import argparse
import fnmatch
import subprocess
import unittest
from pathlib import Path


CI_INFRA = (
    ".github/workflows/ci.yml",
    "scripts/classify-ci-changes.py",
    "scripts/check-foundation-gate.sh",
)
RUST_INFRA = CI_INFRA + (
    "Makefile",
    "scripts/check-rust*",
    "scripts/run-rust-ci-gate.sh",
    "scripts/run-rust-ci-fast.sh",
)
FULL_RUST_MARKERS = RUST_INFRA + (
    "Cargo.lock",
    "Cargo.toml",
    "rust-toolchain.toml",
    "rustfmt.toml",
    "clippy.toml",
    "scripts/check-architecture-boundaries.py",
)
DEV_STACK_FULL = (
    "deploy/dev/**",
    "deploy/compose.spike.yml",
    "deploy/spike/**",
)
DEV_STACK_LITE = (
    "deploy/scripts/**",
    "docs/runbooks/local-development.md",
)
CRATE_SCOPES = {
    "core": ("crates/core/**",),
    "knowledge": (
        "crates/knowledge/**",
        "crates/server/tests/knowledge_consumer.rs",
        "scripts/check-knowledge*",
        "docs/runbooks/knowledge-index-compatibility.md",
    ),
    "server": ("crates/server/**",),
    "cli": ("crates/cli/**",),
    "desktop": ("app/src-tauri/**",),
    "mcp": ("crates/mcp/**",),
}
GROUPS = {
    "rust": FULL_RUST_MARKERS
    + (
        "**/Cargo.toml",
        "crates/**",
        "app/src-tauri/**",
    ),
    "knowledge": CI_INFRA
    + (
        "Cargo.lock",
        "Cargo.toml",
        "crates/server/Cargo.toml",
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
    "frontend": CI_INFRA
    + (
        "package.json",
        "pnpm-lock.yaml",
        "pnpm-workspace.yaml",
        "app/package.json",
        "app/tsconfig.json",
        "app/vite.config.ts",
        "app/eslint.config.js",
        "app/index.html",
        "app/src/**",
    ),
    "web": CI_INFRA
    + (
        "package.json",
        "pnpm-lock.yaml",
        "pnpm-workspace.yaml",
        "web/**",
        "crates/server/openapi/**",
    ),
    "dev_stack": CI_INFRA
    + DEV_STACK_FULL
    + DEV_STACK_LITE,
    "bundle": CI_INFRA
    + (
        ".github/workflows/release-desktop.yml",
        "package.json",
        "pnpm-lock.yaml",
        "app/package.json",
        "app/index.html",
        "app/src-tauri/Cargo.toml",
        "app/src-tauri/build.rs",
        "app/assets/folyvo-logo-icon/**",
        "app/src-tauri/icons/**",
        "app/src-tauri/tauri*.json",
        "app/src-tauri/native-runtime/**",
        "scripts/prepare-desktop-runtime.py",
        "scripts/validate-desktop-bundle.sh",
    ),
    "toolchain": CI_INFRA
    + RUST_INFRA
    + (
        "rust-toolchain.toml",
        "scripts/check-web-toolchain.sh",
        "docs/runbooks/contributor-setup.md",
    ),
    "corpus": CI_INFRA
    + (
        "bench/markhand_web/CORPUS.md",
        "bench/markhand_web/generator-environment.lock.json",
        "bench/markhand_web/requirements-corpus.txt",
        "bench/markhand_web/golden/**",
        "bench/markhand_web/adversarial/**",
        "bench/markhand_web/licenses/**",
        "bench/markhand_web/manifest.lock.json",
        "bench/markhand_web/scripts/generate_corpus.py",
        "scripts/validate_corpus.py",
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


def dev_stack_mode_for(paths: list[str]) -> str:
    full = any(
        fnmatch.fnmatch(path, pattern)
        for path in paths
        for pattern in DEV_STACK_FULL
    )
    if full:
        return "full"
    lite = any(
        fnmatch.fnmatch(path, pattern)
        for path in paths
        for pattern in DEV_STACK_LITE
    )
    if lite:
        return "lite"
    return "full"


def rust_crates_for(paths: list[str]) -> tuple[str, bool]:
    if any(
        fnmatch.fnmatch(path, pattern)
        for path in paths
        for pattern in FULL_RUST_MARKERS
    ):
        return "full", True

    scopes: list[str] = []
    for scope, patterns in CRATE_SCOPES.items():
        if any(
            fnmatch.fnmatch(path, pattern)
            for path in paths
            for pattern in patterns
        ):
            scopes.append(scope)

    if "server" in scopes and "knowledge" not in scopes:
        scopes.insert(0, "knowledge")

    desktop_deps = "desktop" in scopes
    if not scopes:
        return "full", True
    return ",".join(scopes), desktop_deps


def changed_paths(base: str, head: str) -> list[str]:
    if not base or set(base) == {"0"}:
        base = subprocess.check_output(
            ["git", "rev-parse", f"{head}^"], text=True
        ).strip()
    return subprocess.check_output(
        ["git", "diff", "--name-only", "--no-renames", base, head], text=True
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
        self.assertFalse(result["bundle"])
        self.assertFalse(result["toolchain"])
        self.assertFalse(result["corpus"])

    def test_deploy_change_activates_only_dev_stack(self) -> None:
        result = classify(["deploy/dev/compose.yml"])
        self.assertTrue(result["dev_stack"])
        self.assertFalse(result["rust"])
        self.assertFalse(result["frontend"])

    def test_spike_report_only_uses_static_gate(self) -> None:
        self.assertFalse(
            classify(["bench/markhand_web/reports/spike-environment.json"])["dev_stack"]
        )
        self.assertFalse(classify(["scripts/validate_spike.py"])["dev_stack"])
        self.assertFalse(
            classify(["bench/markhand_web/reports/spike-environment.json"])["rust"]
        )

    def test_ci_infra_change_activates_every_group(self) -> None:
        self.assertTrue(all(classify([".github/workflows/ci.yml"]).values()))
        self.assertTrue(all(classify(["scripts/classify-ci-changes.py"]).values()))

    def test_makefile_change_activates_rust_and_toolchain_only(self) -> None:
        result = classify(["Makefile"])
        self.assertTrue(result["rust"])
        self.assertTrue(result["toolchain"])
        self.assertFalse(result["frontend"])
        self.assertFalse(result["web"])
        self.assertFalse(result["dev_stack"])
        self.assertFalse(result["bundle"])
        self.assertFalse(result["corpus"])
        self.assertFalse(result["knowledge"])

    def test_deploy_scripts_use_lite_dev_stack(self) -> None:
        result = classify(["deploy/scripts/init-dev-env.sh"])
        self.assertTrue(result["dev_stack"])
        self.assertEqual(dev_stack_mode_for(["deploy/scripts/init-dev-env.sh"]), "lite")

    def test_deploy_compose_uses_full_dev_stack(self) -> None:
        paths = ["deploy/dev/compose.yml"]
        self.assertTrue(classify(paths)["dev_stack"])
        self.assertEqual(dev_stack_mode_for(paths), "full")

    def test_root_lockfile_activates_both_frontends(self) -> None:
        result = classify(["pnpm-lock.yaml"])
        self.assertTrue(result["frontend"])
        self.assertTrue(result["web"])

    def test_vite_entry_and_server_manifest_activate_required_gates(self) -> None:
        self.assertTrue(classify(["app/index.html"])["frontend"])
        server = classify(["crates/server/Cargo.toml"])
        self.assertTrue(server["rust"])
        self.assertTrue(server["knowledge"])

    def test_packaging_files_activate_linux_bundle(self) -> None:
        self.assertTrue(classify(["app/src-tauri/tauri.conf.json"])["bundle"])
        self.assertTrue(
            classify(["app/assets/folyvo-logo-icon/4a-cam/app-icon.icns"])[
                "bundle"
            ]
        )
        self.assertTrue(classify(["app/package.json"])["bundle"])

    def test_toolchain_checker_activates_toolchain_job(self) -> None:
        result = classify(["scripts/check-web-toolchain.sh"])
        self.assertTrue(result["toolchain"])
        self.assertFalse(result["bundle"])
        rust_toolchain = classify(["rust-toolchain.toml"])
        self.assertTrue(rust_toolchain["rust"])
        self.assertTrue(rust_toolchain["toolchain"])

    def test_corpus_change_activates_strict_corpus_job(self) -> None:
        result = classify(["bench/markhand_web/golden/queries.tsv"])
        self.assertTrue(result["corpus"])
        self.assertFalse(result["rust"])

    def test_server_only_change_scopes_rust_tests(self) -> None:
        crates, desktop = rust_crates_for(["crates/server/src/workers/delete.rs"])
        self.assertEqual(crates, "knowledge,server")
        self.assertFalse(desktop)

    def test_cargo_lock_runs_full_rust_gate(self) -> None:
        crates, desktop = rust_crates_for(["Cargo.lock"])
        self.assertEqual(crates, "full")
        self.assertTrue(desktop)

    def test_desktop_change_requires_desktop_deps(self) -> None:
        crates, desktop = rust_crates_for(["app/src-tauri/src/lib.rs"])
        self.assertEqual(crates, "desktop")
        self.assertTrue(desktop)


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
    rust_crates, rust_desktop_deps = rust_crates_for(paths)
    dev_stack_mode = dev_stack_mode_for(paths) if result["dev_stack"] else "skip"
    if args.github_output:
        with args.github_output.open("a", encoding="utf-8") as output:
            for name, enabled in result.items():
                print(f"{name}={str(enabled).lower()}", file=output)
            print(f"rust_crates={rust_crates}", file=output)
            print(f"rust_desktop_deps={str(rust_desktop_deps).lower()}", file=output)
            print(f"dev_stack_mode={dev_stack_mode}", file=output)
    for name, enabled in result.items():
        print(f"{name}={str(enabled).lower()}")
    print(f"rust_crates={rust_crates}")
    print(f"rust_desktop_deps={str(rust_desktop_deps).lower()}")
    print(f"dev_stack_mode={dev_stack_mode}")
    if paths:
        print("Changed files:", *(f"- {path}" for path in paths), sep="\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
