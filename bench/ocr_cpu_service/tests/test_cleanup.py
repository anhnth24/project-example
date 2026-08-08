from __future__ import annotations

import ast
import subprocess
import tomllib
from pathlib import Path


BENCHMARK_ROOT = Path(__file__).parents[1]
REPOSITORY_ROOT = BENCHMARK_ROOT.parents[1]
PROHIBITED_IMPORTS = {
    "fastapi",
    "httpx",
    "multipart",
    "paddle",
    "paddleocr",
    "paddlex",
    "uvicorn",
}
PROHIBITED_DISTRIBUTIONS = PROHIBITED_IMPORTS | {
    "httpx2",
    "paddlepaddle",
    "python-multipart",
}
EXPECTED_DIRECT_RUNTIME_DEPENDENCIES = {
    "pillow",
    "psutil",
    "pypdfium2",
}
EXPECTED_TEST_DEPENDENCIES = {"pytest"}
REMOVED_TRACKED_PATHS = {
    "bench/ocr_cpu_service/scripts/run_service.sh",
    "bench/ocr_cpu_service/tests/test_api.py",
    "bench/ocr_cpu_service/tests/test_markdown.py",
    "bench/ocr_cpu_service/tests/test_ordering.py",
    "bench/ocr_cpu_service/tests/test_paddle_backend.py",
    "bench/ocr_cpu_service/tests/test_service.py",
}


def _tracked_benchmark_paths(pathspec: str) -> list[Path]:
    result = subprocess.run(
        ["git", "ls-files", pathspec],
        cwd=REPOSITORY_ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    return [
        REPOSITORY_ROOT / path
        for path in result.stdout.splitlines()
        if path
    ]


def test_rejected_service_model_and_launcher_paths_are_untracked() -> None:
    tracked = {
        str(path.relative_to(REPOSITORY_ROOT))
        for path in _tracked_benchmark_paths("bench/ocr_cpu_service/**")
    }
    rejected = sorted(
        path
        for path in tracked
        if path.startswith("bench/ocr_cpu_service/markhand_ocr/")
        or path in REMOVED_TRACKED_PATHS
    )

    assert not rejected, "rejected tracked paths remain:\n" + "\n".join(rejected)


def _normalized_dependency_name(requirement: str) -> str:
    name = requirement.split("[", 1)[0]
    for separator in ("<", ">", "=", "!", "~", ";", " "):
        name = name.split(separator, 1)[0]
    return name.strip().lower().replace("_", "-")


def test_tracked_benchmark_has_no_rejected_runtime_imports() -> None:
    rejected: list[str] = []
    for path in _tracked_benchmark_paths("bench/ocr_cpu_service/*.py"):
        tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
        imported_roots = {
            alias.name.split(".", 1)[0]
            for node in ast.walk(tree)
            if isinstance(node, (ast.Import, ast.ImportFrom))
            for alias in (
                node.names
                if isinstance(node, ast.Import)
                else [ast.alias(name=node.module or "")]
            )
        }
        prohibited = sorted(imported_roots & PROHIBITED_IMPORTS)
        if prohibited:
            rejected.append(
                f"{path.relative_to(REPOSITORY_ROOT)}: {', '.join(prohibited)}"
            )

    assert not rejected, "rejected imports remain:\n" + "\n".join(rejected)


def test_benchmark_dependency_metadata_has_only_retained_packages() -> None:
    project = tomllib.loads(
        (BENCHMARK_ROOT / "pyproject.toml").read_text(encoding="utf-8")
    )
    runtime = {
        _normalized_dependency_name(requirement)
        for requirement in project["project"]["dependencies"]
    }
    optional = project["project"]["optional-dependencies"]
    tests = {
        _normalized_dependency_name(requirement)
        for requirement in optional.get("test", [])
    }
    extra_groups = set(optional) - {"test"}

    lock = tomllib.loads(
        (BENCHMARK_ROOT / "pylock.toml").read_text(encoding="utf-8")
    )
    locked = {
        package["name"].lower().replace("_", "-")
        for package in lock["packages"]
    }

    assert runtime == EXPECTED_DIRECT_RUNTIME_DEPENDENCIES
    assert tests == EXPECTED_TEST_DEPENDENCIES
    assert not extra_groups
    assert not locked & PROHIBITED_DISTRIBUTIONS, (
        "rejected locked distributions remain: "
        + ", ".join(sorted(locked & PROHIBITED_DISTRIBUTIONS))
    )
