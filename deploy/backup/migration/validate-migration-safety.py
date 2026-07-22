#!/usr/bin/env python3
"""Migration safety: immutable checksums + expand→cutover→contract discipline.

Does not modify already-merged migrations. Builds on
`scripts/check-migration-manifest.py` checksum immutability and adds
phase-ordering rules for feature stems that introduce cutover/contract.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import tempfile
import unittest
from collections import defaultdict
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
DEFAULT_DIRECTORY = ROOT / "crates" / "server" / "migrations"
MANIFEST_CHECK = ROOT / "scripts" / "check-migration-manifest.py"

NAME = re.compile(
    r"^(?P<number>\d{4})_(?P<phase>expand|backfill|cutover|contract|index)_"
    r"(?P<stem>[a-z0-9_]+)\.sql$"
)
PHASE_ORDER = {
    "expand": 0,
    "backfill": 1,
    "index": 1,
    "cutover": 2,
    "contract": 3,
}


class MigrationSafetyError(ValueError):
    """Fail-closed migration safety error."""


def parse_migrations(directory: Path) -> list[tuple[int, str, str, Path]]:
    rows: list[tuple[int, str, str, Path]] = []
    for path in sorted(directory.glob("*.sql")):
        match = NAME.fullmatch(path.name)
        if not match:
            raise MigrationSafetyError(f"invalid migration name: {path.name}")
        rows.append(
            (
                int(match.group("number")),
                match.group("phase"),
                match.group("stem"),
                path,
            )
        )
    numbers = [row[0] for row in rows]
    if numbers != sorted(numbers):
        raise MigrationSafetyError("migration numbers must be sorted by filename")
    if len(set(numbers)) != len(numbers):
        raise MigrationSafetyError("duplicate migration sequence numbers")
    return rows


def validate_phase_discipline(rows: list[tuple[int, str, str, Path]]) -> list[str]:
    """Expand → (backfill|index)* → cutover → contract for a given stem."""
    errors: list[str] = []
    by_stem: dict[str, list[tuple[int, str]]] = defaultdict(list)
    for number, phase, stem, _path in rows:
        by_stem[stem].append((number, phase))

    for stem, phases in sorted(by_stem.items()):
        phases_sorted = sorted(phases, key=lambda item: item[0])
        seen_order = [PHASE_ORDER[phase] for _number, phase in phases_sorted]
        if seen_order != sorted(seen_order):
            errors.append(
                f"stem {stem}: phases out of expand→cutover→contract order: "
                f"{[p for _, p in phases_sorted]}"
            )
        names = {phase for _number, phase in phases_sorted}
        if "contract" in names and "expand" not in names:
            errors.append(f"stem {stem}: contract without expand")
        if "cutover" in names and "expand" not in names:
            errors.append(f"stem {stem}: cutover without expand")
        if "contract" in names and "cutover" not in names:
            # Allow contract after expand-only only when an explicit cutover exists.
            errors.append(
                f"stem {stem}: contract requires a prior cutover migration"
            )
        # Monotonic sequence across phases for the stem.
        last_number = -1
        last_order = -1
        for number, phase in phases_sorted:
            order = PHASE_ORDER[phase]
            if number < last_number:
                errors.append(f"stem {stem}: non-monotonic sequence at {number:04d}")
            if order < last_order:
                errors.append(
                    f"stem {stem}: phase regression at {number:04d}_{phase}"
                )
            last_number = number
            last_order = order
    return errors


def validate_checksum_immutability(directory: Path) -> list[str]:
    """Delegate to the published immutable manifest checker."""
    import importlib.util

    spec = importlib.util.spec_from_file_location(
        "check_migration_manifest", MANIFEST_CHECK
    )
    if spec is None or spec.loader is None:
        return [f"cannot load {MANIFEST_CHECK}"]
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    try:
        return list(module.validate(directory))
    except (OSError, ValueError, json.JSONDecodeError) as error:
        return [str(error)]


def validate(directory: Path) -> list[str]:
    errors = validate_checksum_immutability(directory)
    try:
        rows = parse_migrations(directory)
    except MigrationSafetyError as error:
        return errors + [str(error)]
    errors.extend(validate_phase_discipline(rows))
    # Merged migrations must remain expand-only or already-valid chains —
    # never rewrite historical files.
    return errors


class MigrationSafetyTests(unittest.TestCase):
    def _write(self, directory: Path, name: str, body: str = "-- test\n") -> None:
        (directory / name).write_text(body, encoding="utf-8")

    def test_existing_repo_migrations_pass(self) -> None:
        self.assertEqual(validate(DEFAULT_DIRECTORY), [])

    def test_contract_without_cutover_fails(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            self._write(directory, "0001_expand_widgets.sql")
            self._write(directory, "0002_contract_widgets.sql")
            # Minimal manifest for checksum path.
            import importlib.util

            spec = importlib.util.spec_from_file_location(
                "check_migration_manifest", MANIFEST_CHECK
            )
            assert spec and spec.loader
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            (directory / "manifest.json").write_text(
                json.dumps(module.expected_manifest(directory), indent=2) + "\n",
                encoding="utf-8",
            )
            errors = validate(directory)
            self.assertTrue(any("contract requires a prior cutover" in e for e in errors))

    def test_phase_order_regression_fails(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            self._write(directory, "0001_expand_widgets.sql")
            self._write(directory, "0002_cutover_widgets.sql")
            self._write(directory, "0003_expand_widgets_extra.sql")
            # different stem ok; same stem regression:
            self._write(directory, "0004_expand_widgets.sql")
            import importlib.util

            spec = importlib.util.spec_from_file_location(
                "check_migration_manifest", MANIFEST_CHECK
            )
            assert spec and spec.loader
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            # Duplicate stem expand after cutover via higher number — invalid name
            # duplicate stem with expand after cutover:
            # Actually 0004_expand_widgets has same stem as 0001/0002 — order regresses.
            (directory / "manifest.json").write_text(
                json.dumps(module.expected_manifest(directory), indent=2) + "\n",
                encoding="utf-8",
            )
            errors = validate(directory)
            self.assertTrue(
                any("out of expand→cutover→contract order" in e or "phase regression" in e for e in errors),
                errors,
            )

    def test_checksum_mutation_fails(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            path = directory / "0001_expand_widgets.sql"
            path.write_text("-- a\n", encoding="utf-8")
            import importlib.util

            spec = importlib.util.spec_from_file_location(
                "check_migration_manifest", MANIFEST_CHECK
            )
            assert spec and spec.loader
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            (directory / "manifest.json").write_text(
                json.dumps(module.expected_manifest(directory), indent=2) + "\n",
                encoding="utf-8",
            )
            path.write_text("-- mutated\n", encoding="utf-8")
            errors = validate(directory)
            self.assertTrue(any("checksum changed" in e for e in errors))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, default=DEFAULT_DIRECTORY)
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)

    if args.self_test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(MigrationSafetyTests)
        result = unittest.TextTestRunner(verbosity=2).run(suite)
        return 0 if result.wasSuccessful() else 1

    try:
        errors = validate(args.directory)
    except MigrationSafetyError as error:
        print(f"migration safety error: {error}", file=sys.stderr)
        return 2
    if errors:
        print("migration safety check failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    print("migration safety check passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
