#!/usr/bin/env python3
"""SQL lexer that strips comments/strings/dollar-quotes before destructive scans."""

from __future__ import annotations

import re
from typing import Iterable


class SqlLexError(ValueError):
    pass


# Destructive by phase: DROP TABLE/COLUMN, TRUNCATE, DELETE FROM, ALTER…DROP COLUMN.
# DROP CONSTRAINT alone is allowed in expand/index (FK/check rewires).
_DESTRUCTIVE = re.compile(
    r"(?is)\b("
    r"DROP\s+TABLE(?:\s+IF\s+EXISTS)?|"
    r"TRUNCATE\s+TABLE|"
    r"DELETE\s+FROM|"
    r"ALTER\s+TABLE\s+\S+\s+DROP\s+COLUMN|"
    r"DROP\s+COLUMN"
    r")\b"
)


def strip_sql_noise(sql: str) -> str:
    """Remove line/block comments, quoted strings, and dollar-quoted bodies."""
    out: list[str] = []
    i = 0
    n = len(sql)
    while i < n:
        ch = sql[i]
        nxt = sql[i + 1] if i + 1 < n else ""
        # Line comment
        if ch == "-" and nxt == "-":
            i += 2
            while i < n and sql[i] not in "\n\r":
                i += 1
            continue
        # Block comment
        if ch == "/" and nxt == "*":
            i += 2
            while i + 1 < n and not (sql[i] == "*" and sql[i + 1] == "/"):
                i += 1
            i = min(n, i + 2)
            continue
        # Dollar quote
        if ch == "$":
            m = re.match(r"\$([A-Za-z_][A-Za-z0-9_]*)?\$", sql[i:])
            if m:
                tag = m.group(0)
                i += len(tag)
                end = sql.find(tag, i)
                if end < 0:
                    raise SqlLexError("unterminated dollar-quote")
                i = end + len(tag)
                out.append(" ")
                continue
        # Single-quoted string
        if ch == "'":
            i += 1
            while i < n:
                if sql[i] == "'":
                    if i + 1 < n and sql[i + 1] == "'":
                        i += 2
                        continue
                    i += 1
                    break
                i += 1
            else:
                raise SqlLexError("unterminated string literal")
            out.append(" ")
            continue
        # Double-quoted identifier — keep as spaces to avoid keyword false positives inside
        if ch == '"':
            i += 1
            while i < n:
                if sql[i] == '"':
                    if i + 1 < n and sql[i + 1] == '"':
                        i += 2
                        continue
                    i += 1
                    break
                i += 1
            else:
                raise SqlLexError("unterminated quoted identifier")
            out.append(" ")
            continue
        out.append(ch)
        i += 1
    return "".join(out)


def find_destructive_operations(sql: str) -> list[str]:
    cleaned = strip_sql_noise(sql)
    return [m.group(0).upper().replace("\n", " ") for m in _DESTRUCTIVE.finditer(cleaned)]


def assert_phase_allows_sql(phase: str, sql: str) -> Iterable[str]:
    hits = find_destructive_operations(sql)
    if not hits:
        return []
    if phase in {"expand", "cutover", "backfill", "index"}:
        return [f"destructive operation in {phase}: {h}" for h in hits]
    return []
