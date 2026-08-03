#!/usr/bin/env python3
"""Hermetic regression tests for web-e2e-real.sh process orchestration.

The real Playwright upload suite needs convert/index/embedding workers plus
fileconv-server. These tests fail when the harness starts the API alone.
"""

from __future__ import annotations

import re
import sys
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "web-e2e-real.sh"


def _read_script() -> str:
    return SCRIPT.read_text(encoding="utf-8")


class WebE2eRealOrchestrationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.text = _read_script()

    def _require(self, pattern: str, label: str) -> None:
        if not re.search(pattern, self.text, re.MULTILINE | re.DOTALL):
            self.fail(f"{label} (pattern not found in web-e2e-real.sh)")

    def _forbid(self, pattern: str, label: str) -> None:
        if re.search(pattern, self.text, re.MULTILINE | re.IGNORECASE):
            self.fail(label)

    def test_starts_convert_index_embedding_workers(self) -> None:
        self._require(r"MARKHAND_WORKER_KIND=convert", "convert worker kind")
        self._require(r"MARKHAND_WORKER_KIND=index", "index worker kind")
        self._require(r"MARKHAND_WORKER_KIND=embedding", "embedding worker kind")
        self._require(r"fileconv-worker", "fileconv-worker binary")

    def test_builds_repository_fileconv_binary(self) -> None:
        self._require(r"cargo build[^\n]*fileconv-cli", "build fileconv-cli")
        self._require(
            r'MARKHAND_CONVERTER_ARGV_JSON=[^\n]*target/(?:debug|release)/fileconv',
            "converter argv points at repo-built fileconv",
        )

    def test_uses_markhand_worker_database_url(self) -> None:
        self._require(
            r"MARKHAND_WORKER_DATABASE_URL=[^\n]*markhand_worker",
            "dedicated markhand_worker database URL",
        )
        self._forbid(
            r"MARKHAND_WORKER_ALLOW_APP_DB_FALLBACK",
            "must not weaken worker identity with app-role fallback",
        )

    def test_separate_worker_logs_and_redacted_failure_dump(self) -> None:
        self._require(r"worker.*log|convert_log|index_log|embedding_log", "worker log paths")
        self._require(r"redact_secrets\.py", "redacted log dump on failure")

    def test_waits_for_all_child_processes_on_exit(self) -> None:
        self._require(r"worker_pids", "tracks worker pids")
        self._require(r"wait[^\n]*worker", "waits for worker processes on cleanup")

    def test_fail_fast_if_worker_dies_before_playwright(self) -> None:
        self._require(
            r"kill -0[^\n]*worker",
            "readiness loop checks worker processes are still alive",
        )


def main() -> int:
    loader = unittest.TestLoader()
    suite = loader.loadTestsFromModule(sys.modules[__name__])
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    raise SystemExit(main())
