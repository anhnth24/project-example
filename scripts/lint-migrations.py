#!/usr/bin/env python3
"""Static analysis linter for Markhand PostgreSQL migrations.

Catches migration anti-patterns before slow database integration tests:
1. `SET [LOCAL|SESSION] row_security = ...`:
   `SET row_security = off` does NOT bypass RLS; PostgreSQL defines it as
   "raise an error instead of silently filtering", causing 42501 permission
   denied failures on FORCE ROW LEVEL SECURITY tables during backfills.
   Fix: Table owner drops FORCE temporarily during the transaction:
        ALTER TABLE <table> NO FORCE ROW LEVEL SECURITY;
        -- backfill statements...
        ALTER TABLE <table> FORCE ROW LEVEL SECURITY;
2. `DROP TABLE` without explanatory comments (planned rule).
3. Unbounded `DELETE` or `UPDATE` on large tables without `WHERE` (planned rule).
"""

from __future__ import annotations

import argparse
import re
import sys
import tempfile
import unittest
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_DIRECTORY = ROOT / "crates/server/migrations"

SET_ROW_SECURITY_PATTERN = re.compile(
    r"\bSET\s+(?:LOCAL\s+|SESSION\s+)?row_security\b",
    re.IGNORECASE,
)


@dataclass
class LintViolation:
    file: str
    line_no: int
    rule_id: str
    message: str
    line_content: str

    def __str__(self) -> str:
        return f"{self.file}:{self.line_no}: [{self.rule_id}] {self.message}\n    Line: {self.line_content}"


def strip_comments(sql: str) -> list[tuple[int, str]]:
    """Return list of (line_no, code_line) with comments removed."""
    def block_replacer(match: re.Match[str]) -> str:
        return "\n" * match.group(0).count("\n")

    no_blocks = re.sub(r"/\*.*?\*/", block_replacer, sql, flags=re.DOTALL)
    result: list[tuple[int, str]] = []
    for line_no, raw_line in enumerate(no_blocks.splitlines(), start=1):
        cleaned = re.sub(r"--.*$", "", raw_line).strip()
        result.append((line_no, cleaned))
    return result


def lint_content(sql: str, filename: str = "<unknown>") -> list[LintViolation]:
    """Scan SQL migration content for anti-patterns."""
    violations: list[LintViolation] = []
    lines = strip_comments(sql)

    for line_no, line in lines:
        if not line:
            continue
        if SET_ROW_SECURITY_PATTERN.search(line):
            violations.append(
                LintViolation(
                    file=filename,
                    line_no=line_no,
                    rule_id="no-set-row-security",
                    message=(
                        "`SET row_security` does not bypass RLS; PostgreSQL defines it as "
                        "'raise error instead of silently filtering', causing SQLSTATE 42501. "
                        "Temporarily disable FORCE ROW LEVEL SECURITY within the migration transaction instead:\n"
                        "    ALTER TABLE <table> NO FORCE ROW LEVEL SECURITY;\n"
                        "    -- backfill statements...\n"
                        "    ALTER TABLE <table> FORCE ROW LEVEL SECURITY;"
                    ),
                    line_content=line,
                )
            )

    return violations


def lint_directory(directory: Path) -> list[LintViolation]:
    """Scan all .sql migration files in the target directory."""
    if not directory.is_dir():
        raise FileNotFoundError(f"Migrations directory not found: {directory}")

    violations: list[LintViolation] = []
    for path in sorted(directory.glob("*.sql")):
        content = path.read_text(encoding="utf-8")
        violations.extend(lint_content(content, filename=path.name))
    return violations


class LintMigrationsTests(unittest.TestCase):
    def test_valid_migration_passes(self) -> None:
        """Valid migration using ALTER TABLE ... NO FORCE RLS and safe comments passes."""
        valid_sql = """-- Migration 0037: backfill chunks
-- Note: SET LOCAL row_security = off does NOT work.
/* Another comment referencing SET row_security = off */
ALTER TABLE chunks NO FORCE ROW LEVEL SECURITY;
UPDATE chunks SET body = body;
ALTER TABLE chunks FORCE ROW LEVEL SECURITY;
"""
        violations = lint_content(valid_sql, "0037_valid.sql")
        self.assertEqual(len(violations), 0)

    def test_detects_set_local_row_security(self) -> None:
        """Original broken migration 0037 pattern is detected."""
        invalid_sql = """-- Migration 0037 broken pattern
SET LOCAL row_security = off;
UPDATE chunks SET body = body;
"""
        violations = lint_content(invalid_sql, "0037_invalid.sql")
        self.assertEqual(len(violations), 1)
        self.assertEqual(violations[0].line_no, 2)
        self.assertEqual(violations[0].rule_id, "no-set-row-security")
        self.assertEqual(violations[0].line_content, "SET LOCAL row_security = off;")

    def test_detects_all_set_row_security_variants(self) -> None:
        """Handles various case and whitespace variants of SET row_security."""
        variants = [
            "SET row_security = off;",
            "set local row_security = off;",
            "SET SESSION row_security = 'off';",
            "set  local   row_security  to  off;",
            "  SET  row_security = DEFAULT; ",
        ]
        for variant in variants:
            with self.subTest(variant=variant):
                violations = lint_content(variant, "test.sql")
                self.assertEqual(len(violations), 1, f"Failed to match variant: {variant}")
                self.assertEqual(violations[0].rule_id, "no-set-row-security")

    def test_ignores_comments(self) -> None:
        """Comment lines mentioning SET row_security do not cause false positives."""
        comment_sql = """-- SET LOCAL row_security = off;
-- SET row_security = off;
/* SET LOCAL row_security = off; */
/*
Multi-line comment
SET LOCAL row_security = off;
*/
SELECT 1;
"""
        violations = lint_content(comment_sql, "comments.sql")
        self.assertEqual(len(violations), 0)

    def test_directory_scan(self) -> None:
        """Directory scanner reports violations across multiple files."""
        with tempfile.TemporaryDirectory() as tmp_dir:
            dir_path = Path(tmp_dir)
            (dir_path / "0001_good.sql").write_text("SELECT 1;\n", encoding="utf-8")
            (dir_path / "0002_bad.sql").write_text(
                "SET LOCAL row_security = off;\n", encoding="utf-8"
            )

            violations = lint_directory(dir_path)
            self.assertEqual(len(violations), 1)
            self.assertEqual(violations[0].file, "0002_bad.sql")
            self.assertEqual(violations[0].line_no, 1)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--directory",
        type=Path,
        default=DEFAULT_DIRECTORY,
        help="Path to migration directory (default: crates/server/migrations)",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="Lint migration files (default behavior)",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="Run internal unit tests",
    )
    args = parser.parse_args()

    if args.self_test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(LintMigrationsTests)
        runner = unittest.TextTestRunner(verbosity=2)
        result = runner.run(suite)
        return 0 if result.wasSuccessful() else 1

    try:
        violations = lint_directory(args.directory)
    except Exception as exc:
        print(f"migration lint error: {exc}", file=sys.stderr)
        return 1

    if violations:
        print(f"SQL migration lint failed with {len(violations)} violation(s):", file=sys.stderr)
        for v in violations:
            print(f"- {v}", file=sys.stderr)
        return 1

    file_count = len(list(args.directory.glob("*.sql")))
    print(f"SQL migration lint passed ({file_count} files checked).")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
