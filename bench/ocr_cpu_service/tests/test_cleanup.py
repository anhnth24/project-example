from __future__ import annotations

import ast
import copy
import re
import subprocess
import tomllib
from pathlib import Path
from urllib.parse import urlsplit

import pytest


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
EXPECTED_BUILD_DEPENDENCIES = {"setuptools"}
EXPECTED_LOCKED_DISTRIBUTIONS = {
    "iniconfig",
    "packaging",
    "pillow",
    "pluggy",
    "psutil",
    "pygments",
    "pypdfium2",
    "pytest",
    "setuptools",
}
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


def _validate_dependency_lock(lock: dict[str, object]) -> None:
    packages = lock.get("packages")
    assert isinstance(packages, list), "lock packages must be a list"
    names = [
        package["name"].lower().replace("_", "-")
        for package in packages
        if isinstance(package, dict) and isinstance(package.get("name"), str)
    ]
    assert len(names) == len(packages), "every lock package must have a name"
    assert len(names) == len(set(names)), "lock package names must be unique"
    assert set(names) == EXPECTED_LOCKED_DISTRIBUTIONS, (
        "lock must contain the exact allowed resolved distribution set"
    )

    for package in packages:
        assert isinstance(package, dict)
        assert "sdist" not in package, f"{package['name']}: lock must be wheel-only"
        wheels = package.get("wheels")
        assert isinstance(wheels, list) and wheels, (
            f"{package['name']}: lock must contain at least one wheel"
        )
        for wheel in wheels:
            assert isinstance(wheel, dict)
            url = wheel.get("url")
            assert isinstance(url, str) and urlsplit(url).scheme == "https", (
                f"{package['name']}: wheel URL must use HTTPS"
            )
            assert urlsplit(url).path.endswith(".whl"), (
                f"{package['name']}: URL must reference a wheel"
            )
            hashes = wheel.get("hashes")
            assert isinstance(hashes, dict) and set(hashes) == {"sha256"}, (
                f"{package['name']}: wheel must have only a SHA-256 hash"
            )
            assert isinstance(hashes["sha256"], str) and re.fullmatch(
                r"[0-9a-fA-F]{64}", hashes["sha256"]
            ), f"{package['name']}: SHA-256 must be 64 hexadecimal characters"


def _valid_lock_fixture() -> dict[str, object]:
    return {
        "lock-version": "1.0",
        "packages": [
            {
                "name": name,
                "version": "1.0",
                "wheels": [
                    {
                        "name": f"{name}-1.0-py3-none-any.whl",
                        "url": (
                            f"https://files.example.test/"
                            f"{name}-1.0-py3-none-any.whl"
                        ),
                        "hashes": {"sha256": f"{index:064x}"},
                    }
                ],
            }
            for index, name in enumerate(
                sorted(EXPECTED_LOCKED_DISTRIBUTIONS), start=1
            )
        ],
    }


@pytest.mark.parametrize(
    ("malformation", "message"),
    [
        ("unexpected-package", "exact allowed"),
        ("duplicate-package", "unique"),
        ("sdist", "wheel-only"),
        ("http-url", "HTTPS"),
        ("non-wheel-url", "reference a wheel"),
        ("missing-hash", "only a SHA-256"),
        ("invalid-hash", "64 hexadecimal"),
    ],
)
def test_dependency_lock_rejects_malformed_fixtures(
    malformation: str, message: str
) -> None:
    lock = copy.deepcopy(_valid_lock_fixture())
    packages = lock["packages"]
    assert isinstance(packages, list)
    first = packages[0]
    assert isinstance(first, dict)
    wheels = first["wheels"]
    assert isinstance(wheels, list)
    wheel = wheels[0]
    assert isinstance(wheel, dict)

    if malformation == "unexpected-package":
        first["name"] = "requests"
    elif malformation == "duplicate-package":
        packages.append(copy.deepcopy(first))
    elif malformation == "sdist":
        first["sdist"] = {
            "name": "source.tar.gz",
            "url": "https://files.example.test/source.tar.gz",
            "hashes": {"sha256": "0" * 64},
        }
    elif malformation == "http-url":
        wheel["url"] = str(wheel["url"]).replace("https://", "http://")
    elif malformation == "non-wheel-url":
        wheel["url"] = "https://files.example.test/source.tar.gz"
    elif malformation == "missing-hash":
        wheel["hashes"] = {}
    elif malformation == "invalid-hash":
        wheel["hashes"] = {"sha256": "not-a-sha256"}
    else:
        raise AssertionError(f"unhandled malformation: {malformation}")

    with pytest.raises(AssertionError, match=message):
        _validate_dependency_lock(lock)


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
    build = {
        _normalized_dependency_name(requirement)
        for requirement in optional.get("build", [])
    }

    lock = tomllib.loads(
        (BENCHMARK_ROOT / "pylock.toml").read_text(encoding="utf-8")
    )

    assert runtime == EXPECTED_DIRECT_RUNTIME_DEPENDENCIES
    assert tests == EXPECTED_TEST_DEPENDENCIES
    assert build == EXPECTED_BUILD_DEPENDENCIES
    assert set(optional) == {"build", "test"}
    _validate_dependency_lock(lock)
