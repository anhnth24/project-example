#!/usr/bin/env python3
"""Reject Clippy warnings that are not part of the approved legacy baseline."""

from __future__ import annotations

import json
import subprocess
import sys
import unittest
from collections import Counter


LEGACY_WARNINGS = Counter(
    {
        ("clippy::while_let_loop", "crates/core/src/audio.rs"): 1,
        ("clippy::manual_range_contains", "crates/core/src/chunk.rs"): 1,
        ("clippy::needless_range_loop", "crates/core/src/conv/csv_conv.rs"): 1,
        ("clippy::field_reassign_with_default", "crates/core/src/conv/pdf.rs"): 1,
        ("clippy::uninlined_format_args", "crates/core/src/conv/xlsx.rs"): 1,
        ("clippy::io_other_error", "crates/core/src/image_ocr.rs"): 4,
        ("clippy::manual_ignore_case_cmp", "crates/core/src/intelligence.rs"): 1,
        ("clippy::too_many_arguments", "crates/core/src/intelligence.rs"): 1,
        ("clippy::uninlined_format_args", "crates/core/src/intelligence.rs"): 1,
        ("clippy::needless_lifetimes", "crates/core/src/intelligence.rs"): 1,
        ("clippy::uninlined_format_args", "app/src-tauri/src/intelligence.rs"): 2,
        ("clippy::field_reassign_with_default", "app/src-tauri/src/lib.rs"): 5,
    }
)


def warning_key(message: dict) -> tuple[str, str, int] | None:
    if message.get("level") != "warning":
        return None
    code = message.get("code") or {}
    if not str(code.get("code", "")).startswith("clippy::"):
        return None
    spans = message.get("spans") or []
    primary = next((span for span in spans if span.get("is_primary")), None)
    if primary is None:
        return None
    return code["code"], primary["file_name"], primary["line_start"]


def clippy_warnings(lines: list[str]) -> Counter[tuple[str, str]]:
    warnings: Counter[tuple[str, str]] = Counter()
    seen: set[tuple[str, str, int]] = set()
    for line in lines:
        record = json.loads(line)
        if record.get("reason") != "compiler-message":
            continue
        key = warning_key(record["message"])
        if key and key not in seen:
            seen.add(key)
            code, file_name, _line = key
            warnings[(code, file_name)] += 1
    return warnings


class BaselineTests(unittest.TestCase):
    def test_reads_primary_clippy_span(self) -> None:
        line = json.dumps(
            {
                "reason": "compiler-message",
                "message": {
                    "level": "warning",
                    "code": {"code": "clippy::example"},
                    "spans": [
                        {"file_name": "a.rs", "line_start": 1, "is_primary": True}
                    ],
                },
            }
        )
        self.assertEqual(clippy_warnings([line]), Counter({("clippy::example", "a.rs"): 1}))

    def test_count_baseline_rejects_additional_warning_but_allows_line_shift(self) -> None:
        current = Counter({("clippy::example", "a.rs"): 2})
        allowed = Counter({("clippy::example", "a.rs"): 1})
        self.assertEqual(current - allowed, Counter({("clippy::example", "a.rs"): 1}))


def main() -> int:
    if "--self-test" in sys.argv:
        return 0 if unittest.main(argv=[sys.argv[0]], exit=False).result.wasSuccessful() else 1

    result = subprocess.run(
        ["cargo", "clippy", "--workspace", "--all-targets", "--message-format=json"],
        check=False,
        text=True,
        capture_output=True,
    )
    sys.stderr.write(result.stderr)
    if result.returncode:
        return result.returncode
    unexpected = clippy_warnings(result.stdout.splitlines()) - LEGACY_WARNINGS
    if unexpected:
        print("new Clippy warning(s) outside legacy baseline:", file=sys.stderr)
        for (code, file_name), count in sorted(unexpected.items()):
            print(f"- {file_name}: {code} (+{count})", file=sys.stderr)
        return 1
    print("Clippy legacy baseline has no new warnings")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
