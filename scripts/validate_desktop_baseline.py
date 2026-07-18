#!/usr/bin/env python3
"""Validate committed P0-03 desktop baseline evidence."""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import re
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
CORPUS = ROOT / "bench/markhand_web"
DEFAULT_BASELINE = CORPUS / "baselines/desktop-v1"
SHA256 = re.compile(r"^[0-9a-f]{64}$")
UNSAFE = (
    re.compile(r"/(?:workspace|home|Users|tmp)/"),
    re.compile(r"\b[A-Za-z]:\\"),
    re.compile(r"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----"),
)


def load(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def ranking_sha(rows: list[dict]) -> str:
    payload = [
        [row["queryId"], row["rankedDocuments"]]
        for row in rows
    ]
    return hashlib.sha256(
        json.dumps(
            payload,
            ensure_ascii=False,
            separators=(",", ":"),
        ).encode()
    ).hexdigest()


def validate(path: Path) -> list[str]:
    errors: list[str] = []
    required = (
        "metadata.json",
        "conversion-results.json",
        "retrieval-results.json",
        "retrieval-raw.json",
        "desktop-baseline.md",
    )
    for name in required:
        if not (path / name).is_file():
            errors.append(f"baseline missing {name}")
    if errors:
        return errors
    metadata = load(path / "metadata.json")
    conversions = load(path / "conversion-results.json").get("documents", [])
    retrieval = load(path / "retrieval-results.json")
    raw = load(path / "retrieval-raw.json")
    manifest = load(CORPUS / "golden/manifest.json")
    with (CORPUS / "golden/queries.tsv").open(encoding="utf-8", newline="") as source:
        queries = list(csv.DictReader(source, delimiter="\t"))

    expected_documents = {item["id"] for item in manifest["documents"]}
    actual_documents = {item.get("documentId") for item in conversions}
    if actual_documents != expected_documents or len(conversions) != len(expected_documents):
        errors.append("conversion evidence does not cover every corpus document")
    formats = {item["format"] for item in manifest["documents"]}
    if {item.get("format") for item in conversions} != formats:
        errors.append("conversion evidence does not cover every format")
    for item in conversions:
        if item.get("status") not in {"ok", "error"}:
            errors.append(f"conversion {item.get('documentId')}: invalid status")
        if item.get("status") == "ok":
            if not isinstance(item.get("cer"), (int, float)) or not isinstance(
                item.get("wer"), (int, float)
            ):
                errors.append(f"conversion {item.get('documentId')}: missing metrics")
        elif item.get("cer") is not None or item.get("wer") is not None:
            errors.append(f"conversion {item.get('documentId')}: failed metrics must be null")
        raw_path = path / "raw" / f"{item.get('documentId')}.md"
        if not raw_path.is_file():
            errors.append(f"conversion {item.get('documentId')}: raw Markdown missing")

    expected_queries = {row["query_id"] for row in queries}
    result_rows = retrieval.get("queries", [])
    raw_rows = raw.get("queries", [])
    if {row.get("queryId") for row in result_rows} != expected_queries:
        errors.append("retrieval results do not cover every query")
    if {row.get("queryId") for row in raw_rows} != expected_queries:
        errors.append("raw retrieval output does not cover every query")
    judged_count = sum(bool(json.loads(row["judgments"])) for row in queries)
    no_answer_count = len(queries) - judged_count
    if retrieval.get("summary", {}).get("queries") != judged_count:
        errors.append("retrieval summary query count mismatch")
    if retrieval.get("noAnswerSummary", {}).get("queries") != no_answer_count:
        errors.append("no-answer summary query count mismatch")
    temporal = retrieval.get("temporalSummary", {})
    if temporal.get("versionCitationPrecision") != 0.0 or temporal.get(
        "versionCitationRecall"
    ) != 0.0:
        errors.append("desktop baseline must record missing version citation payload")
    if metadata.get("retrievalRankingSha256") != ranking_sha(result_rows):
        errors.append("retrieval ranking fingerprint mismatch")
    if not re.fullmatch(r"[0-9a-f]{40}", str(metadata.get("gitCommit", ""))):
        errors.append("metadata gitCommit is invalid")
    if metadata.get("gitDirty") is not False:
        errors.append("baseline was not measured from a clean tracked tree")
    for field in ("fixtureManifestSha256", "converterSha256", "retrievalRankingSha256"):
        if not SHA256.fullmatch(str(metadata.get(field, ""))):
            errors.append(f"metadata {field} is invalid")

    for evidence in path.rglob("*"):
        if evidence.is_file() and evidence.stat().st_size <= 10 * 1024 * 1024:
            text = evidence.read_text(encoding="utf-8", errors="ignore")
            if any(pattern.search(text) for pattern in UNSAFE):
                errors.append(f"baseline contains unsafe path/secret: {evidence.name}")
    return errors


class BaselineValidatorTests(unittest.TestCase):
    def test_ranking_fingerprint_changes_with_order(self) -> None:
        first = [{"queryId": "q1", "rankedDocuments": ["a", "b"]}]
        second = [{"queryId": "q1", "rankedDocuments": ["b", "a"]}]
        self.assertNotEqual(ranking_sha(first), ranking_sha(second))

    def test_missing_baseline_fails(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            errors = validate(Path(temporary))
            self.assertTrue(any("missing" in error for error in errors))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--baseline", type=Path, default=DEFAULT_BASELINE)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(BaselineValidatorTests)
        return 0 if unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful() else 1
    errors = validate(args.baseline.resolve())
    if errors:
        for error in errors:
            print(f"- {error}")
        return 1
    print("P0-03 desktop baseline validation passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
