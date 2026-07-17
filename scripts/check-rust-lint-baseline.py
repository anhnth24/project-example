#!/usr/bin/env python3
"""Reject Clippy warnings that are not part of the approved legacy baseline."""

from __future__ import annotations

import json
import subprocess
import sys
import unittest


LEGACY_WARNINGS = {
    ("clippy::while_let_loop", "crates/core/src/audio.rs", 228),
    ("clippy::manual_range_contains", "crates/core/src/chunk.rs", 43),
    ("clippy::needless_range_loop", "crates/core/src/conv/csv_conv.rs", 125),
    ("clippy::field_reassign_with_default", "crates/core/src/conv/pdf.rs", 185),
    ("clippy::uninlined_format_args", "crates/core/src/conv/xlsx.rs", 35),
    ("clippy::io_other_error", "crates/core/src/image_ocr.rs", 98),
    ("clippy::io_other_error", "crates/core/src/image_ocr.rs", 262),
    ("clippy::io_other_error", "crates/core/src/image_ocr.rs", 378),
    ("clippy::io_other_error", "crates/core/src/image_ocr.rs", 435),
    ("clippy::manual_ignore_case_cmp", "crates/core/src/intelligence.rs", 1087),
    ("clippy::too_many_arguments", "crates/core/src/intelligence.rs", 1109),
    ("clippy::uninlined_format_args", "crates/core/src/intelligence.rs", 1317),
    ("clippy::needless_lifetimes", "crates/core/src/intelligence.rs", 1339),
    ("clippy::uninlined_format_args", "app/src-tauri/src/intelligence.rs", 237),
    ("clippy::uninlined_format_args", "app/src-tauri/src/intelligence.rs", 446),
    ("clippy::field_reassign_with_default", "app/src-tauri/src/lib.rs", 1293),
    ("clippy::field_reassign_with_default", "app/src-tauri/src/lib.rs", 1302),
    ("clippy::field_reassign_with_default", "app/src-tauri/src/lib.rs", 1314),
    ("clippy::field_reassign_with_default", "app/src-tauri/src/lib.rs", 1327),
    ("clippy::field_reassign_with_default", "app/src-tauri/src/lib.rs", 1341),
}


def warning_key(message: dict) -> tuple[str | None, str, int] | None:
    if message.get("level") != "warning":
        return None
    code = message.get("code") or {}
    if not str(code.get("code", "")).startswith("clippy::"):
        return None
    spans = message.get("spans") or []
    primary = next((span for span in spans if span.get("is_primary")), None)
    if primary is None:
        return None
    return code.get("code"), primary["file_name"], primary["line_start"]


def clippy_warnings(lines: list[str]) -> set[tuple[str | None, str, int]]:
    warnings = set()
    for line in lines:
        record = json.loads(line)
        if record.get("reason") != "compiler-message":
            continue
        key = warning_key(record["message"])
        if key:
            warnings.add(key)
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
        self.assertEqual(clippy_warnings([line]), {("clippy::example", "a.rs", 1)})


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
    unexpected = sorted(clippy_warnings(result.stdout.splitlines()) - LEGACY_WARNINGS)
    if unexpected:
        print("new Clippy warning(s) outside legacy baseline:", file=sys.stderr)
        for code, file_name, line in unexpected:
            print(f"- {file_name}:{line}: {code}", file=sys.stderr)
        return 1
    print("Clippy legacy baseline has no new warnings")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
