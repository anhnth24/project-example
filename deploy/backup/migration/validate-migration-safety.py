#!/usr/bin/env python3
"""Migration safety: immutable checksums + expand→cutover→contract + SQL lexer."""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
import unittest
from collections import defaultdict
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
DEFAULT_DIRECTORY = ROOT / "crates" / "server" / "migrations"
MANIFEST_CHECK = ROOT / "scripts" / "check-migration-manifest.py"
LIB = Path(__file__).resolve().parents[1] / "lib"
if str(LIB) not in sys.path:
    sys.path.insert(0, str(LIB))

from sql_lexer import SqlLexError, assert_phase_allows_sql, find_destructive_operations  # noqa: E402

import re

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
            errors.append(f"stem {stem}: contract requires a prior cutover migration")
        last_number = -1
        last_order = -1
        for number, phase in phases_sorted:
            order = PHASE_ORDER[phase]
            if number < last_number:
                errors.append(f"stem {stem}: non-monotonic sequence at {number:04d}")
            if order < last_order:
                errors.append(f"stem {stem}: phase regression at {number:04d}_{phase}")
            last_number = number
            last_order = order
    return errors


def validate_checksum_immutability(directory: Path) -> list[str]:
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


def validate_sql_semantics(rows: list[tuple[int, str, str, Path]]) -> list[str]:
    errors: list[str] = []
    stems_with_cutover = {stem for _n, phase, stem, _p in rows if phase == "cutover"}
    for _number, phase, stem, path in rows:
        sql = path.read_text(encoding="utf-8")
        try:
            for msg in assert_phase_allows_sql(phase, sql):
                errors.append(f"{path.name}: {msg}")
        except SqlLexError as error:
            errors.append(f"{path.name}: SQL lex error: {error}")
        if phase == "contract" and stem not in stems_with_cutover:
            errors.append(f"{path.name}: contract without cutover evidence for stem {stem}")
    return errors


def _git_ref_exists(reference: str) -> bool:
    return (
        subprocess.run(
            ["git", "rev-parse", "--verify", "--quiet", f"{reference}^{{commit}}"],
            cwd=ROOT,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        ).returncode
        == 0
    )


def _github_event_base_sha() -> str | None:
    event_path = os.environ.get("GITHUB_EVENT_PATH", "").strip()
    if not event_path:
        return None
    try:
        event = json.loads(Path(event_path).read_text(encoding="utf-8"))
        value = event.get("pull_request", {}).get("base", {}).get("sha")
    except (OSError, json.JSONDecodeError, AttributeError):
        return None
    return value if isinstance(value, str) and value.strip() else None


def resolve_base_ref(explicit: str | None) -> str | None:
    if explicit:
        return explicit
    for key in ("MARKHAND_MIGRATION_BASE_REF", "GITHUB_BASE_SHA"):
        value = os.environ.get(key, "").strip()
        if value:
            return value
    if event_sha := _github_event_base_sha():
        return event_sha
    # actions/checkout may fetch the base commit without creating its remote-tracking
    # branch. Only use GITHUB_BASE_REF when the corresponding ref is resolvable.
    base_name = os.environ.get("GITHUB_BASE_REF", "").strip()
    if base_name:
        for candidate in (f"origin/{base_name}", base_name):
            if _git_ref_exists(candidate):
                return candidate
    # Safe local/CI default: merge-base with origin/master or master.
    for candidate in ("origin/master", "master", "origin/main", "main"):
        try:
            out = subprocess.check_output(
                ["git", "merge-base", "HEAD", candidate],
                cwd=ROOT,
                text=True,
                stderr=subprocess.DEVNULL,
            ).strip()
            if out:
                return out
        except (OSError, subprocess.CalledProcessError):
            continue
    return None


def validate_base_ref_anchor(
    directory: Path, base_ref: str | None, *, required: bool
) -> list[str]:
    if not base_ref:
        if required:
            return [
                "migration base-ref anchor mandatory but unresolved "
                "(set MARKHAND_MIGRATION_BASE_REF / GITHUB_BASE_REF or pass --base-ref)"
            ]
        return []
    errors: list[str] = []
    try:
        raw = subprocess.check_output(
            ["git", "show", f"{base_ref}:crates/server/migrations/manifest.json"],
            cwd=ROOT,
            text=True,
        )
        base_manifest = json.loads(raw)
    except (OSError, subprocess.CalledProcessError, json.JSONDecodeError) as error:
        return [f"base-ref {base_ref} manifest unavailable: {error}"]
    current = json.loads((directory / "manifest.json").read_text(encoding="utf-8"))
    base_migrations = base_manifest.get("migrations") or {}
    current_migrations = current.get("migrations") or {}
    for name, digest in base_migrations.items():
        if name not in current_migrations:
            errors.append(f"base-ref anchor missing migration in working tree: {name}")
        elif current_migrations[name] != digest:
            errors.append(f"base-ref checksum drift vs {base_ref}: {name}")
    return errors


def validate(
    directory: Path,
    base_ref: str | None = None,
    *,
    require_base_ref: bool = True,
) -> list[str]:
    errors = validate_checksum_immutability(directory)
    try:
        rows = parse_migrations(directory)
    except MigrationSafetyError as error:
        return errors + [str(error)]
    errors.extend(validate_phase_discipline(rows))
    errors.extend(validate_sql_semantics(rows))
    resolved = resolve_base_ref(base_ref)
    errors.extend(
        validate_base_ref_anchor(directory, resolved, required=require_base_ref)
    )
    return errors


class MigrationSafetyTests(unittest.TestCase):
    def _write(self, directory: Path, name: str, body: str = "-- test\n") -> None:
        (directory / name).write_text(body, encoding="utf-8")

    def _manifest(self, directory: Path) -> None:
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

    def test_existing_repo_migrations_pass(self) -> None:
        self.assertEqual(validate(DEFAULT_DIRECTORY), [])

    def test_contract_without_cutover_fails(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            self._write(directory, "0001_expand_widgets.sql")
            self._write(directory, "0002_contract_widgets.sql")
            self._manifest(directory)
            errors = validate(directory, require_base_ref=False)
            self.assertTrue(any("contract requires a prior cutover" in e for e in errors))

    def test_phase_order_regression_fails(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            self._write(directory, "0001_expand_widgets.sql")
            self._write(directory, "0002_cutover_widgets.sql")
            self._write(directory, "0003_expand_widgets_extra.sql")
            self._write(directory, "0004_expand_widgets.sql")
            self._manifest(directory)
            errors = validate(directory, require_base_ref=False)
            self.assertTrue(
                any(
                    "out of expand→cutover→contract order" in e or "phase regression" in e
                    for e in errors
                ),
                errors,
            )

    def test_checksum_mutation_fails(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            path = directory / "0001_expand_widgets.sql"
            path.write_text("-- a\n", encoding="utf-8")
            self._manifest(directory)
            path.write_text("-- mutated\n", encoding="utf-8")
            errors = validate(directory, require_base_ref=False)
            self.assertTrue(any("checksum changed" in e for e in errors))

    def test_destructive_sql_in_expand_fails(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            self._write(
                directory,
                "0001_expand_widgets.sql",
                "CREATE TABLE widgets(id int);\nDROP TABLE widgets;\n",
            )
            self._manifest(directory)
            errors = validate(directory, require_base_ref=False)
            self.assertTrue(any("destructive" in e for e in errors), errors)

    def test_literal_and_comment_bypass_ignored(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            self._write(
                directory,
                "0001_expand_widgets.sql",
                "-- DROP TABLE evil;\n"
                "CREATE TABLE widgets(id int);\n"
                "COMMENT ON TABLE widgets IS 'DROP TABLE widgets';\n"
                "DO $$ BEGIN\n"
                "  PERFORM 'DROP COLUMN x';\n"
                "END $$;\n",
            )
            self._manifest(directory)
            errors = validate(directory, require_base_ref=False)
            self.assertFalse(any("destructive" in e for e in errors), errors)

    def test_drop_column_detected_outside_literals(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            self._write(
                directory,
                "0001_expand_widgets.sql",
                "ALTER TABLE widgets DROP COLUMN legacy;\n",
            )
            self._manifest(directory)
            errors = validate(directory, require_base_ref=False)
            self.assertTrue(any("DROP COLUMN" in e.upper() for e in errors), errors)

    def test_missing_base_ref_fails_when_required(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            self._write(directory, "0001_expand_widgets.sql")
            self._manifest(directory)
            env = os.environ.copy()
            for key in (
                "MARKHAND_MIGRATION_BASE_REF",
                "GITHUB_BASE_REF",
                "GITHUB_BASE_SHA",
            ):
                env.pop(key, None)
            # Force unresolved by pointing git to empty.
            errors = validate_base_ref_anchor(directory, None, required=True)
            self.assertTrue(any("mandatory" in e for e in errors))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, default=DEFAULT_DIRECTORY)
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument(
        "--base-ref",
        help="Git ref whose migration manifest.json anchors immutable checksums",
    )
    parser.add_argument(
        "--allow-missing-base-ref",
        action="store_true",
        help="Unsafe: skip mandatory base-ref (tests only)",
    )
    args = parser.parse_args(argv)

    if args.self_test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(MigrationSafetyTests)
        result = unittest.TextTestRunner(verbosity=2).run(suite)
        return 0 if result.wasSuccessful() else 1

    try:
        errors = validate(
            args.directory,
            base_ref=args.base_ref,
            require_base_ref=not args.allow_missing_base_ref,
        )
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
