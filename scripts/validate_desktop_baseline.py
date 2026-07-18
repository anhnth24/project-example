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
    re.compile(r"(?:^|[\"'\s:])/(?:[A-Za-z0-9_.-]+/)+"),
    re.compile(r"\b[A-Za-z]:\\"),
    re.compile(r"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----"),
    re.compile(r"\bAKIA[0-9A-Z]{16}\b"),
    re.compile(r"\b(?:ghp|github_pat)_[A-Za-z0-9_]{20,}\b"),
    re.compile(r"\bsk-[A-Za-z0-9_-]{20,}\b"),
    re.compile(r"\bpostgres(?:ql)?://[^/\s:@]+:[^@\s/]+@"),
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
    published_report = CORPUS / "reports/desktop-baseline.md"
    if not published_report.is_file() or published_report.read_bytes() != (
        path / "desktop-baseline.md"
    ).read_bytes():
        errors.append("published desktop baseline report is missing or stale")
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
    scenarios = raw.get("scenarios", {})
    if {row.get("queryId") for row in result_rows} != expected_queries:
        errors.append("retrieval results do not cover every query")
    if {row.get("queryId") for row in raw_rows} != expected_queries:
        errors.append("raw retrieval output does not cover every query")
    fallback = scenarios.get("providerFallback", {})
    mismatch = scenarios.get("signatureMismatch", {})
    restore = scenarios.get("restoreLocal", {})
    if fallback.get("embeddingMode") != "local_hash_v1" or not any(
        "rebuild" in warning for warning in fallback.get("warnings", [])
    ):
        errors.append("provider fallback scenario is not frozen")
    if mismatch.get("embeddingMode") != "provider_v1" or not any(
        "signature" in warning.lower() for warning in mismatch.get("warnings", [])
    ):
        errors.append("signature mismatch scenario is not frozen")
    if restore.get("embeddingMode") != "local_hash_v1":
        errors.append("local restore scenario is not frozen")
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
    raw_fingerprint = hashlib.sha256(
        json.dumps(
            raw,
            ensure_ascii=False,
            sort_keys=True,
            separators=(",", ":"),
        ).encode()
    ).hexdigest()
    if metadata.get("rawRetrievalSha256") != raw_fingerprint:
        errors.append("raw retrieval fingerprint mismatch")
    if metadata.get("retrievalDeterministic") is not True:
        errors.append("independent retrieval rerun was not deterministic")
    if not re.fullmatch(r"[0-9a-f]{40}", str(metadata.get("gitCommit", ""))):
        errors.append("metadata gitCommit is invalid")
    if metadata.get("gitDirty") is not False:
        errors.append("baseline was not measured from a clean tracked tree")
    for field in (
        "fixtureManifestSha256",
        "converterSha256",
        "retrievalRankingSha256",
        "rawRetrievalSha256",
        "composeFileSha256",
    ):
        if not SHA256.fullmatch(str(metadata.get(field, ""))):
            errors.append(f"metadata {field} is invalid")
    if metadata.get("command") != "make p0-desktop-baseline":
        errors.append("metadata command is not reproducible")
    hardware = metadata.get("hardware", {})
    if (
        not isinstance(hardware.get("cpu", {}).get("threads"), int)
        or hardware["cpu"]["threads"] < 1
        or not isinstance(hardware.get("ramGb"), (int, float))
        or hardware["ramGb"] <= 0
    ):
        errors.append("metadata hardware fingerprint is incomplete")
    native = metadata.get("nativeRuntime", {})
    if native.get("pdfium", {}).get("present") is not True or not SHA256.fullmatch(
        str(native.get("pdfium", {}).get("sha256", ""))
    ):
        errors.append("metadata PDFium fingerprint is incomplete")
    if native.get("tesseract", {}).get("present") is not True or not native.get(
        "tesseract", {}
    ).get("languageModelSha256", {}).get("vie"):
        errors.append("metadata Tesseract Vietnamese fingerprint is incomplete")

    for evidence in path.rglob("*"):
        if evidence.is_file():
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
