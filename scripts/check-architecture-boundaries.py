#!/usr/bin/env python3
"""Enforce the dependency baseline from docs/adr/0001-web-boundaries.md."""

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
FORBIDDEN_CORE = {
    "tauri",
    "axum",
    "sqlx",
    "rusqlite",
    "qdrant-client",
    "aws-sdk-s3",
    "s3",
    "minio",
}
FORBIDDEN_KNOWLEDGE = (FORBIDDEN_CORE - {"rusqlite"}) | {
    "fileconv-desktop",
    "fileconv-server",
}
FORBIDDEN_WEB_PATTERNS = (
    r"""from\s+["']@tauri-apps/""",
    r"""require\(\s*["']@tauri-apps/""",
    r"window\.__TAURI__",
)
DIRECT_ROUTE_IO = (
    r"\bsqlx::",
    r"\brusqlite::",
    r"\bqdrant",
    r"\baws_sdk_s3",
    r"\bminio",
)


def direct_dependencies(manifest: Path) -> set[str]:
    metadata = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1", "--manifest-path", str(manifest)],
        check=True,
        capture_output=True,
        text=True,
    )
    package = json.loads(metadata.stdout)["packages"][0]
    return {dependency["name"] for dependency in package["dependencies"]}


def rust_files(path: Path) -> list[Path]:
    return list(path.rglob("*.rs")) if path.is_dir() else []


def validate(root: Path, *, cargo_dependencies=direct_dependencies) -> list[str]:
    failures: list[str] = []
    vendor = root / "vendor/markitdown-rs"
    if vendor.exists():
        workspace = (root / "Cargo.toml").read_text(encoding="utf-8")
        if "vendor/markitdown-rs" not in workspace or "exclude" not in workspace:
            failures.append("vendor/markitdown-rs phải bị exclude khỏi Cargo workspace")

    for crate, forbidden in (
        ("core", FORBIDDEN_CORE),
        ("knowledge", FORBIDDEN_KNOWLEDGE),
    ):
        manifest = root / f"crates/{crate}/Cargo.toml"
        if not manifest.is_file():
            continue
        found = cargo_dependencies(manifest) & forbidden
        if found:
            failures.append(
                f"crates/{crate} có forbidden direct dependencies: {', '.join(sorted(found))}"
            )

    knowledge_manifest = root / "crates/knowledge/Cargo.toml"
    if knowledge_manifest.is_file():
        content = knowledge_manifest.read_text(encoding="utf-8")
        for dependency in ("rusqlite", "hnsw_rs"):
            declaration = next(
                (line for line in content.splitlines() if line.startswith(f"{dependency} =")),
                "",
            )
            if declaration and "optional = true" not in declaration:
                failures.append(
                    f"crates/knowledge desktop dependency must be optional: {dependency}"
                )

    desktop_knowledge = root / "app/src-tauri/src/knowledge.rs"
    if desktop_knowledge.is_file():
        content = desktop_knowledge.read_text(encoding="utf-8")
        for forbidden in (
            "rusqlite::",
            "hnsw_rs::",
            "fn cosine(",
            "fn hybrid_rerank_score(",
            "fn extractive_answer(",
            "fn answer_is_grounded(",
            "CREATE TABLE",
            "chunks_fts MATCH",
        ):
            if forbidden in content:
                failures.append(
                    f"desktop knowledge adapter contains extracted implementation: {forbidden}"
                )
        for required in (
            "service::rebuild_index",
            "service::hybrid_search",
            "service::index_stats",
            "service::grounded_answer",
        ):
            if required not in content:
                failures.append(f"desktop knowledge adapter does not delegate {required}")
        if (root / "app/src-tauri/src/vector_index.rs").exists():
            failures.append("legacy desktop vector_index.rs must remain extracted")

    knowledge_service = root / "crates/knowledge/src/desktop/service.rs"
    if knowledge_service.is_file():
        content = knowledge_service.read_text(encoding="utf-8")
        for forbidden in ("tauri::", "AppState", "data_root", "resolve_within", "Settings"):
            if forbidden in content:
                failures.append(
                    f"knowledge desktop service imports app boundary: {forbidden}"
                )

    server_manifest = root / "crates/server/Cargo.toml"
    if server_manifest.is_file() and "fileconv-desktop" in cargo_dependencies(server_manifest):
        failures.append("crates/server không được depend ngược vào fileconv-desktop")

    web = root / "web"
    if web.is_dir():
        for source in [*web.rglob("*.ts"), *web.rglob("*.tsx")]:
            content = source.read_text(encoding="utf-8")
            if any(re.search(pattern, content) for pattern in FORBIDDEN_WEB_PATTERNS):
                failures.append(f"{source.relative_to(root)} import Tauri/browser desktop API")

    routes = root / "crates/server/src/routes"
    for source in rust_files(routes):
        content = source.read_text(encoding="utf-8")
        if any(re.search(pattern, content, re.IGNORECASE) for pattern in DIRECT_ROUTE_IO):
            failures.append(f"{source.relative_to(root)} truy cập DB/storage trực tiếp; dùng service")

    return failures


class BoundaryCheckTests(unittest.TestCase):
    def test_accepts_core_without_framework_or_storage(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "Cargo.toml").write_text('[workspace]\nexclude = ["vendor/markitdown-rs"]\n')
            (root / "vendor/markitdown-rs").mkdir(parents=True)
            manifest = root / "crates/core/Cargo.toml"
            manifest.parent.mkdir(parents=True)
            manifest.write_text("[package]\nname = \"fileconv-core\"\nversion = \"0.1.0\"\n")
            self.assertEqual(validate(root, cargo_dependencies=lambda _: set()), [])

    def test_rejects_tauri_in_core(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "Cargo.toml").write_text('[workspace]\nexclude = ["vendor/markitdown-rs"]\n')
            (root / "vendor/markitdown-rs").mkdir(parents=True)
            manifest = root / "crates/core/Cargo.toml"
            manifest.parent.mkdir(parents=True)
            manifest.write_text("[package]\nname = \"fileconv-core\"\nversion = \"0.1.0\"\n")
            failures = validate(root, cargo_dependencies=lambda _: {"tauri"})
            self.assertTrue(any("forbidden" in failure for failure in failures))

    def test_rejects_tauri_import_in_web(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "Cargo.toml").write_text('[workspace]\nexclude = ["vendor/markitdown-rs"]\n')
            (root / "vendor/markitdown-rs").mkdir(parents=True)
            source = root / "web/src/api.ts"
            source.parent.mkdir(parents=True)
            source.write_text('import { invoke } from "@tauri-apps/api/core";\n')
            failures = validate(root, cargo_dependencies=lambda _: set())
            self.assertTrue(any("import Tauri" in failure for failure in failures))

    def test_rejects_non_optional_desktop_dependency_in_knowledge(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "Cargo.toml").write_text('[workspace]\nexclude = ["vendor/markitdown-rs"]\n')
            manifest = root / "crates/knowledge/Cargo.toml"
            manifest.parent.mkdir(parents=True)
            manifest.write_text(
                '[package]\nname = "fileconv-knowledge"\nversion = "0.1.0"\n'
                '[dependencies]\nrusqlite = "0.37"\n'
            )
            failures = validate(root, cargo_dependencies=lambda _: {"rusqlite"})
            self.assertTrue(any("must be optional" in failure for failure in failures))

    def test_rejects_fat_desktop_knowledge_adapter(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "Cargo.toml").write_text("[workspace]\n")
            adapter = root / "app/src-tauri/src/knowledge.rs"
            adapter.parent.mkdir(parents=True)
            adapter.write_text("fn cosine() {} // rusqlite::Connection\n")
            failures = validate(root, cargo_dependencies=lambda _: set())
            self.assertTrue(any("extracted implementation" in failure for failure in failures))
            self.assertTrue(any("does not delegate" in failure for failure in failures))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(BoundaryCheckTests)
        return 0 if unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful() else 1

    failures = validate(ROOT)
    if failures:
        print("architecture boundary check failed:", file=sys.stderr)
        for failure in failures:
            print(f"- {failure}", file=sys.stderr)
        return 1
    print("architecture boundary check passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
