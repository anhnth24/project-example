#!/usr/bin/env python3
"""Validate deterministic, licensed and non-sensitive fixture manifests."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
import tempfile
import unittest
from pathlib import Path, PurePosixPath


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_ROOT = ROOT / "tests/fixtures"
SHA256 = re.compile(r"^[0-9a-f]{64}$")
SECRET_CANARIES = (
    re.compile(rb"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----"),
    re.compile(rb"\bAKIA[0-9A-Z]{16}\b"),
    re.compile(rb"\b(?:ghp|github_pat)_[A-Za-z0-9_]{20,}\b"),
    re.compile(rb"\bpostgres(?:ql)?://[^/\s:@]+:[^@\s/]+@"),
)
ABSOLUTE_CONTENT_PATHS = (
    re.compile(rb"(?:^|\s)/(?:home|Users|workspace|tmp)/[^\s]+"),
    re.compile(rb"\b[A-Za-z]:\\(?:Users|Temp|workspace)\\"),
)


def checksum(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def safe_fixture_path(root: Path, raw: str) -> Path:
    pure = PurePosixPath(raw)
    if pure.is_absolute() or ".." in pure.parts or raw != pure.as_posix():
        raise ValueError(f"fixture path must be normalized and relative: {raw}")
    resolved = (root / pure).resolve()
    if not resolved.is_relative_to(root.resolve()):
        raise ValueError(f"fixture path escapes root: {raw}")
    return resolved


def validate(root: Path) -> list[str]:
    manifest_path = root / "manifest.json"
    data = json.loads(manifest_path.read_text(encoding="utf-8"))
    if data.get("version") != 1 or not isinstance(data.get("fixtures"), list):
        raise ValueError("manifest requires version=1 and fixtures array")

    errors: list[str] = []
    ids: set[str] = set()
    paths: set[str] = set()
    for fixture in data["fixtures"]:
        fixture_id = fixture.get("id")
        raw_path = fixture.get("path")
        if not isinstance(fixture_id, str) or not fixture_id:
            errors.append("fixture has missing id")
            continue
        if fixture_id in ids:
            errors.append(f"duplicate fixture id: {fixture_id}")
        ids.add(fixture_id)
        if not isinstance(raw_path, str):
            errors.append(f"{fixture_id}: missing path")
            continue
        try:
            path = safe_fixture_path(root, raw_path)
        except ValueError as error:
            errors.append(f"{fixture_id}: {error}")
            continue
        if raw_path in paths:
            errors.append(f"duplicate fixture path: {raw_path}")
        paths.add(raw_path)
        if not path.is_file():
            errors.append(f"{fixture_id}: fixture missing: {raw_path}")
            continue
        expected = fixture.get("sha256")
        if not isinstance(expected, str) or not SHA256.fullmatch(expected):
            errors.append(f"{fixture_id}: invalid sha256")
        elif checksum(path) != expected:
            errors.append(f"{fixture_id}: checksum mismatch")
        for required in ("kind", "owner", "source", "license"):
            if not isinstance(fixture.get(required), str) or not fixture[required].strip():
                errors.append(f"{fixture_id}: missing {required}")
        if fixture.get("sensitive") is not False:
            errors.append(f"{fixture_id}: sensitive must be false")
        content = path.read_bytes()
        if any(pattern.search(content) for pattern in SECRET_CANARIES):
            errors.append(f"{fixture_id}: secret canary detected")
        if any(pattern.search(content) for pattern in ABSOLUTE_CONTENT_PATHS):
            errors.append(f"{fixture_id}: absolute machine path detected")

    unmanaged = sorted(
        path.relative_to(root).as_posix()
        for path in root.rglob("*")
        if path.is_file()
        and path.name not in {"README.md", "manifest.json"}
        and path.relative_to(root).as_posix() not in paths
    )
    errors.extend(f"unmanaged fixture file: {path}" for path in unmanaged)
    return errors


class FixtureValidatorTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def write_manifest(self, fixtures: list[dict]) -> None:
        (self.root / "manifest.json").write_text(
            json.dumps({"version": 1, "fixtures": fixtures})
        )

    def fixture(self, fixture_id: str, path: str, content: bytes) -> dict:
        target = self.root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(content)
        return {
            "id": fixture_id,
            "path": path,
            "sha256": checksum(target),
            "kind": "text",
            "owner": "test",
            "source": "synthetic",
            "license": "CC0-1.0",
            "sensitive": False,
        }

    def test_accepts_valid_fixture(self) -> None:
        fixture = self.fixture("valid", "sample.txt", b"synthetic")
        self.write_manifest([fixture])
        self.assertEqual(validate(self.root), [])

    def test_catches_duplicate_checksum_absolute_path_and_secret(self) -> None:
        first = self.fixture("duplicate", "one.txt", b"safe")
        second = self.fixture("duplicate", "two.txt", b"ghp_AAAAAAAAAAAAAAAAAAAAAAAA")
        first["sha256"] = "0" * 64
        second["path"] = "/home/user/two.txt"
        self.write_manifest([first, second])
        errors = validate(self.root)
        self.assertTrue(any("duplicate fixture id" in error for error in errors))
        self.assertTrue(any("checksum mismatch" in error for error in errors))
        self.assertTrue(any("normalized and relative" in error for error in errors))

    def test_catches_secret_and_absolute_path_content(self) -> None:
        fixture = self.fixture(
            "unsafe",
            "unsafe.txt",
            b"postgres://user:password@host/db /home/alice/private.txt",
        )
        self.write_manifest([fixture])
        errors = validate(self.root)
        self.assertTrue(any("secret canary" in error for error in errors))
        self.assertTrue(any("absolute machine path" in error for error in errors))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=DEFAULT_ROOT)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(FixtureValidatorTests)
        return 0 if unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful() else 1
    try:
        errors = validate(args.root)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"fixture validation error: {error}", file=sys.stderr)
        return 1
    if errors:
        print("fixture validation failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    print("fixture validation passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
