#!/usr/bin/env python3
"""Validate immutable Markhand server migration filenames and checksums."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_DIRECTORY = ROOT / "crates/server/migrations"
NAME = re.compile(r"^(?P<number>\d{4})_(?:expand|backfill|cutover|contract)_[a-z0-9_]+\.sql$")


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def migration_files(directory: Path) -> list[Path]:
    files = sorted(directory.glob("*.sql"))
    numbers: set[int] = set()
    for path in files:
        match = NAME.fullmatch(path.name)
        if not match:
            raise ValueError(f"invalid migration name: {path.name}")
        number = int(match.group("number"))
        if number in numbers:
            raise ValueError(f"duplicate migration sequence: {number:04d}")
        numbers.add(number)
    return files


def expected_manifest(directory: Path) -> dict[str, object]:
    return {
        "version": 1,
        "migrations": {path.name: digest(path) for path in migration_files(directory)},
    }


def load_manifest(directory: Path) -> dict[str, object]:
    path = directory / "manifest.json"
    if not path.is_file():
        raise ValueError("missing migration manifest.json")
    value = json.loads(path.read_text(encoding="utf-8"))
    if value.get("version") != 1 or not isinstance(value.get("migrations"), dict):
        raise ValueError("manifest must contain version=1 and migrations object")
    return value


def validate(directory: Path) -> list[str]:
    expected = expected_manifest(directory)
    actual = load_manifest(directory)
    if actual == expected:
        return []
    expected_migrations = expected["migrations"]
    actual_migrations = actual["migrations"]
    errors: list[str] = []
    for name in sorted(set(expected_migrations) | set(actual_migrations)):
        if name not in actual_migrations:
            errors.append(f"unmanifested migration: {name}")
        elif name not in expected_migrations:
            errors.append(f"manifest references missing migration: {name}")
        elif actual_migrations[name] != expected_migrations[name]:
            errors.append(f"checksum changed: {name}")
    return errors


class MigrationManifestTests(unittest.TestCase):
    def test_detects_checksum_mutation_and_invalid_name(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            migration = directory / "0001_expand_organizations.sql"
            migration.write_text("-- phase: 1B\nCREATE TABLE organizations (); \n")
            (directory / "manifest.json").write_text(
                json.dumps(expected_manifest(directory))
            )
            self.assertEqual(validate(directory), [])
            migration.write_text("-- phase: 1B\nCREATE TABLE organizations (id uuid); \n")
            self.assertEqual(validate(directory), ["checksum changed: 0001_expand_organizations.sql"])
            (directory / "bad-name.sql").write_text("SELECT 1;\n")
            with self.assertRaisesRegex(ValueError, "invalid migration name"):
                validate(directory)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--directory", type=Path, default=DEFAULT_DIRECTORY)
    parser.add_argument("--check", action="store_true", help="Validate manifest (default).")
    parser.add_argument("--write-manifest", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(MigrationManifestTests)
        return 0 if unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful() else 1

    if args.write_manifest:
        args.directory.joinpath("manifest.json").write_text(
            json.dumps(expected_manifest(args.directory), indent=2) + "\n",
            encoding="utf-8",
        )
        print(f"wrote {args.directory / 'manifest.json'}")
        return 0

    try:
        errors = validate(args.directory)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"migration manifest error: {error}", file=sys.stderr)
        return 1
    if errors:
        print("migration manifest check failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    print("migration manifest check passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
