#!/usr/bin/env python3
"""Validate committed P0-03 desktop baseline evidence."""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import re
import shutil
import tempfile
import unittest
import unicodedata
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
    re.compile(r"\bFILECONV_(?:LLM|EMBEDDING)_API_KEY\s*=\s*\S+"),
    re.compile(r"\bAuthorization\s*:\s*Bearer\s+\S+", re.IGNORECASE),
    re.compile(r'"(?:apiKey|api_key|token)"\s*:\s*"[^"]{8,}"'),
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


def normalize(value: str) -> str:
    cleaned = []
    for line in unicodedata.normalize("NFC", value).splitlines():
        stripped = line.strip()
        if stripped and all(character in "|-: " for character in stripped):
            continue
        cleaned.append(line.replace("|", " "))
    return " ".join(
        token
        for token in " ".join(cleaned).split()
        if not all(character == "#" or character == "-" for character in token)
    )


def distance(left: list[str], right: list[str]) -> int:
    if len(left) > len(right):
        left, right = right, left
    previous = list(range(len(left) + 1))
    for row, right_item in enumerate(right, 1):
        current = [row]
        for column, left_item in enumerate(left, 1):
            current.append(
                min(
                    current[-1] + 1,
                    previous[column] + 1,
                    previous[column - 1] + (left_item != right_item),
                )
            )
        previous = current
    return previous[-1]


def error_rates(expected: str, actual: str) -> tuple[float, float]:
    expected_normal = normalize(expected)
    actual_normal = normalize(actual)
    expected_chars = list(expected_normal)
    actual_chars = list(actual_normal)
    expected_words = expected_normal.split()
    actual_words = actual_normal.split()
    return (
        distance(expected_chars, actual_chars) / max(1, len(expected_chars)),
        distance(expected_words, actual_words) / max(1, len(expected_words)),
    )


def discounted_gain(grades: list[int]) -> float:
    return sum(
        (2**grade - 1) / math.log2(index + 2)
        for index, grade in enumerate(grades)
    )


def close(actual: object, expected: float) -> bool:
    return isinstance(actual, (int, float)) and abs(actual - expected) <= 1e-6


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

    manifest_by_id = {item["id"]: item for item in manifest["documents"]}
    expected_documents = set(manifest_by_id)
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
            continue
        actual = raw_path.read_text(encoding="utf-8")
        corpus_item = manifest_by_id.get(item.get("documentId"))
        if corpus_item is None:
            continue
        expected = (
            CORPUS / "golden" / corpus_item["markdownPath"]
        ).read_text(encoding="utf-8")
        if item.get("actualChars") != len(actual):
            errors.append(f"conversion {item.get('documentId')}: actualChars mismatch")
        if item.get("expectedChars") != len(expected):
            errors.append(f"conversion {item.get('documentId')}: expectedChars mismatch")
        if item.get("actualSha256") != hashlib.sha256(actual.encode()).hexdigest():
            errors.append(f"conversion {item.get('documentId')}: raw hash mismatch")
        if item.get("status") == "ok":
            cer, wer = error_rates(expected, actual)
            if not close(item.get("cer"), cer):
                errors.append(f"conversion {item.get('documentId')}: CER mismatch")
            if not close(item.get("wer"), wer):
                errors.append(f"conversion {item.get('documentId')}: WER mismatch")

    expected_queries = {row["query_id"] for row in queries}
    query_by_id = {row["query_id"]: row for row in queries}
    result_rows = retrieval.get("queries", [])
    raw_rows = raw.get("queries", [])
    scenarios = raw.get("scenarios", {})
    if {row.get("queryId") for row in result_rows} != expected_queries:
        errors.append("retrieval results do not cover every query")
    if {row.get("queryId") for row in raw_rows} != expected_queries:
        errors.append("raw retrieval output does not cover every query")
    raw_by_id = {row["queryId"]: row for row in raw_rows}
    recomputed_rows = []
    for row in result_rows:
        query_id = row.get("queryId")
        query = query_by_id.get(query_id)
        raw_row = raw_by_id.get(query_id)
        if query is None or raw_row is None:
            continue
        judgments = json.loads(query["judgments"])
        relevant = {doc for doc, grade in judgments.items() if grade >= 2}
        raw_ranked = list(
            dict.fromkeys(hit["sourceRel"] for hit in raw_row.get("hits", []))
        )
        ranked = row.get("rankedDocuments", [])
        if ranked != raw_ranked:
            errors.append(f"{query_id}: derived ranking differs from raw hits")
        recall5 = (
            len(relevant.intersection(ranked[:5])) / len(relevant)
            if relevant
            else 0.0
        )
        recall10 = (
            len(relevant.intersection(ranked[:10])) / len(relevant)
            if relevant
            else 0.0
        )
        hit5 = 1.0 if relevant and relevant.intersection(ranked[:5]) else 0.0
        reciprocal = next(
            (
                1 / (index + 1)
                for index, document in enumerate(ranked)
                if document in relevant
            ),
            0.0,
        )
        actual_grades = [judgments.get(document, 0) for document in ranked[:10]]
        ideal_grades = sorted(judgments.values(), reverse=True)[:10]
        ideal = discounted_gain(ideal_grades)
        ndcg = discounted_gain(actual_grades) / ideal if ideal else 1.0
        for field, value in (
            ("recallAt5", recall5),
            ("recallAt10", recall10),
            ("hitAt5", hit5),
            ("reciprocalRank", reciprocal),
            ("ndcgAt10", ndcg),
        ):
            if not close(row.get(field), value):
                errors.append(f"{query_id}: {field} is miscomputed")
        citation_tokens = [
            int(value)
            for value in re.findall(
                r"\[CITE-(\d{4})\]", raw_row.get("answer", "")
            )
        ]
        valid_tokens = [
            value
            for value in citation_tokens
            if 1 <= value <= len(raw_row.get("hits", []))
        ]
        emitted_hits = [raw_row["hits"][value - 1] for value in valid_tokens]
        actual_citation_docs = {hit["sourceRel"] for hit in emitted_hits}
        expected_citations = json.loads(query["citations"])
        expected_citation_docs = {
            citation["documentId"] for citation in expected_citations
        }
        precision = (
            len(actual_citation_docs & expected_citation_docs)
            / len(actual_citation_docs)
            if actual_citation_docs
            else 1.0 if not expected_citation_docs else 0.0
        )
        recall = (
            len(actual_citation_docs & expected_citation_docs)
            / len(expected_citation_docs)
            if expected_citation_docs
            else 1.0 if not actual_citation_docs else 0.0
        )
        if not close(row.get("citationDocumentPrecision"), precision):
            errors.append(f"{query_id}: citation precision is miscomputed")
        if not close(row.get("citationDocumentRecall"), recall):
            errors.append(f"{query_id}: citation recall is miscomputed")
        emitted_by_document = {}
        for hit in emitted_hits:
            emitted_by_document.setdefault(hit["sourceRel"], hit)
        matched = [
            citation
            for citation in expected_citations
            if citation["documentId"] in emitted_by_document
        ]
        paged = [citation for citation in matched if citation["page"] is not None]
        page_accuracy = (
            sum(
                emitted_by_document[citation["documentId"]]["anchor"]["page"]
                == citation["page"]
                for citation in paged
            )
            / len(paged)
            if paged
            else None
        )
        span_accuracy = (
            sum(
                emitted_by_document[citation["documentId"]]["anchor"]["start"]
                == citation["start"]
                and emitted_by_document[citation["documentId"]]["anchor"]["end"]
                == citation["end"]
                for citation in matched
            )
            / len(matched)
            if matched
            else 0.0
        )
        if page_accuracy is None:
            if row.get("citationPageAccuracy") is not None:
                errors.append(f"{query_id}: citation page accuracy is miscomputed")
        elif not close(row.get("citationPageAccuracy"), page_accuracy):
            errors.append(f"{query_id}: citation page accuracy is miscomputed")
        if not close(row.get("citationSpanExactAccuracy"), span_accuracy):
            errors.append(f"{query_id}: citation span accuracy is miscomputed")
        token_validity = len(valid_tokens) / max(1, len(citation_tokens))
        if not close(row.get("citationTokenValidity"), token_validity):
            errors.append(f"{query_id}: citation token validity is miscomputed")
        answer_exact = normalize(raw_row.get("answer", "")) == normalize(
            query["expected_answer"]
        )
        answer_contains = bool(query["expected_answer"]) and normalize(
            query["expected_answer"]
        ) in normalize(raw_row.get("answer", ""))
        if row.get("answerExact") is not answer_exact:
            errors.append(f"{query_id}: answerExact is miscomputed")
        if row.get("answerContainsExpected") is not answer_contains:
            errors.append(f"{query_id}: answerContainsExpected is miscomputed")
        recomputed_rows.append(
            {
                "queryId": query_id,
                "category": query["category"],
                "versionMode": query["version_mode"],
                "topDocument": ranked[0] if ranked else None,
                "expectedDocument": query["expected_doc"] or None,
                "hasRelevant": bool(relevant),
                "recallAt5": recall5,
                "recallAt10": recall10,
                "hitAt5": hit5,
                "mrr": reciprocal,
                "ndcg": ndcg,
                "citationPrecision": precision,
                "citationRecall": recall,
                "pageAccuracy": page_accuracy,
                "spanAccuracy": span_accuracy,
                "tokenValidity": token_validity,
                "answerExact": answer_exact,
                "answerContains": answer_contains,
            }
        )
    fallback = scenarios.get("providerFallback", {})
    mismatch = scenarios.get("signatureMismatch", {})
    query_mismatch = scenarios.get("querySignatureMismatch", {})
    restore = scenarios.get("restoreLocal", {})
    if fallback.get("embeddingMode") != "local_hash_v1" or not any(
        "rebuild" in warning for warning in fallback.get("warnings", [])
    ):
        errors.append("provider fallback scenario is not frozen")
    if mismatch.get("embeddingMode") != "provider_v1" or not any(
        "signature" in warning.lower() for warning in mismatch.get("warnings", [])
    ):
        errors.append("signature mismatch scenario is not frozen")
    if query_mismatch.get("embeddingMode") != "provider_v1" or not any(
        "không khớp index" in warning
        for warning in query_mismatch.get("warnings", [])
    ):
        errors.append("query-time signature mismatch fallback is not frozen")
    if restore.get("embeddingMode") != "local_hash_v1":
        errors.append("local restore scenario is not frozen")
    judged_count = sum(bool(json.loads(row["judgments"])) for row in queries)
    no_answer_count = len(queries) - judged_count

    def aggregate(items: list[dict]) -> dict:
        relevant = [row for row in items if row["hasRelevant"]]
        return {
            "queries": len(relevant),
            "recallAt5": sum(row["recallAt5"] for row in relevant)
            / max(1, len(relevant)),
            "recallAt10": sum(row["recallAt10"] for row in relevant)
            / max(1, len(relevant)),
            "hitAt5": sum(row["hitAt5"] for row in relevant)
            / max(1, len(relevant)),
            "mrr": sum(row["mrr"] for row in relevant) / max(1, len(relevant)),
            "ndcgAt10": sum(row["ndcg"] for row in relevant)
            / max(1, len(relevant)),
        }

    if retrieval.get("summary", {}).get("queries") != judged_count:
        errors.append("retrieval summary query count mismatch")
    judged_rows = [row for row in recomputed_rows if row["hasRelevant"]]
    summary = retrieval.get("summary", {})
    for field, key in (
        ("recallAt5", "recallAt5"),
        ("recallAt10", "recallAt10"),
        ("hitAt5", "hitAt5"),
        ("mrr", "mrr"),
        ("ndcgAt10", "ndcg"),
    ):
        expected_value = sum(row[key] for row in judged_rows) / max(
            1, len(judged_rows)
        )
        if not close(summary.get(field), expected_value):
            errors.append(f"retrieval summary {field} is miscomputed")
    categories = {}
    for row in recomputed_rows:
        categories.setdefault(row["category"], []).append(row)
    if set(retrieval.get("categories", {})) != set(categories):
        errors.append("retrieval category set is incomplete")
    for category, items in categories.items():
        expected_category = aggregate(items)
        actual_category = retrieval.get("categories", {}).get(category, {})
        for field, value in expected_category.items():
            if field == "queries":
                if actual_category.get(field) != value:
                    errors.append(f"category {category} query count is incorrect")
            elif not close(actual_category.get(field), value):
                errors.append(f"category {category} {field} is miscomputed")
    citation_summary = retrieval.get("citationSummary", {})
    paged_rows = [
        row for row in judged_rows if row["pageAccuracy"] is not None
    ]
    citation_expectations = {
        "documentPrecision": sum(row["citationPrecision"] for row in recomputed_rows)
        / max(1, len(recomputed_rows)),
        "documentRecall": sum(row["citationRecall"] for row in recomputed_rows)
        / max(1, len(recomputed_rows)),
        "pageAccuracy": sum(row["pageAccuracy"] for row in paged_rows)
        / max(1, len(paged_rows)),
        "spanExactAccuracy": sum(row["spanAccuracy"] for row in judged_rows)
        / max(1, len(judged_rows)),
        "tokenValidity": sum(row["tokenValidity"] for row in recomputed_rows)
        / max(1, len(recomputed_rows)),
        "answerExactAccuracy": sum(row["answerExact"] for row in recomputed_rows)
        / max(1, len(recomputed_rows)),
        "answerContainsExpectedAccuracy": sum(
            row["answerContains"] for row in recomputed_rows
        )
        / max(1, len(recomputed_rows)),
    }
    for field, value in citation_expectations.items():
        if not close(citation_summary.get(field), value):
            errors.append(f"citation summary {field} is miscomputed")
    if retrieval.get("noAnswerSummary", {}).get("queries") != no_answer_count:
        errors.append("no-answer summary query count mismatch")
    raw_no_answer = [
        raw_by_id[row["query_id"]]
        for row in queries
        if not json.loads(row["judgments"])
    ]
    no_answer_accuracy = sum(not row.get("hits") for row in raw_no_answer) / max(
        1, len(raw_no_answer)
    )
    if not close(
        retrieval.get("noAnswerSummary", {}).get("accuracy"),
        no_answer_accuracy,
    ):
        errors.append("no-answer accuracy is miscomputed")
    temporal = retrieval.get("temporalSummary", {})
    temporal_rows = [
        row
        for row in judged_rows
        if row["versionMode"] != "current"
        or row["category"] in {"temporal_current", "conflict_current"}
    ]
    expected_temporal = aggregate(temporal_rows)
    for field, value in expected_temporal.items():
        if field == "queries":
            if temporal.get(field) != value:
                errors.append("temporal summary query count is incorrect")
        elif not close(temporal.get(field), value):
            errors.append(f"temporal summary {field} is miscomputed")
    current_temporal = [
        row
        for row in temporal_rows
        if row["category"] in {"temporal_current", "conflict_current"}
    ]
    current_accuracy = sum(
        row["topDocument"] == row["expectedDocument"]
        for row in current_temporal
    ) / max(1, len(current_temporal))
    if not close(temporal.get("currentVersionTop1Accuracy"), current_accuracy):
        errors.append("current-version Top-1 accuracy is miscomputed")
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
    conversion_projection = [
        [
            item["documentId"],
            item["status"],
            item["exitCode"],
            item["actualChars"],
            item["actualSha256"],
        ]
        for item in conversions
    ]
    conversion_fingerprint = hashlib.sha256(
        json.dumps(conversion_projection, separators=(",", ":")).encode()
    ).hexdigest()
    if metadata.get("conversionRerunSha256") != conversion_fingerprint:
        errors.append("conversion rerun fingerprint mismatch")
    if metadata.get("conversionDeterministic") is not True:
        errors.append("independent conversion rerun was not deterministic")
    if not re.fullmatch(r"[0-9a-f]{40}", str(metadata.get("gitCommit", ""))):
        errors.append("metadata gitCommit is invalid")
    if metadata.get("gitDirty") is not False:
        errors.append("baseline was not measured from a clean tracked tree")
    for field in (
        "fixtureManifestSha256",
        "converterSha256",
        "retrievalRankingSha256",
        "rawRetrievalSha256",
        "conversionRerunSha256",
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

    def test_detects_raw_metric_and_secret_mutation(self) -> None:
        self.assertTrue(DEFAULT_BASELINE.is_dir())
        with tempfile.TemporaryDirectory() as temporary:
            baseline = Path(temporary) / "desktop-v1"
            shutil.copytree(DEFAULT_BASELINE, baseline)
            conversions = load(baseline / "conversion-results.json")
            successful = next(
                item
                for item in conversions["documents"]
                if item["status"] == "ok"
            )
            successful["cer"] = 0.999
            (baseline / "conversion-results.json").write_text(
                json.dumps(conversions)
            )
            raw_markdown = baseline / "raw" / f"{successful['documentId']}.md"
            raw_markdown.write_text(raw_markdown.read_text() + " mutated")
            retrieval = load(baseline / "retrieval-results.json")
            first_category = next(iter(retrieval["categories"].values()))
            first_category["recallAt5"] = 0.999
            (baseline / "retrieval-results.json").write_text(
                json.dumps(retrieval)
            )
            metadata = load(baseline / "metadata.json")
            metadata["leak"] = "FILECONV_LLM_API_KEY=synthetic-secret-value"
            (baseline / "metadata.json").write_text(json.dumps(metadata))
            errors = validate(baseline)
            self.assertTrue(any("CER mismatch" in error for error in errors))
            self.assertTrue(any("raw hash mismatch" in error for error in errors))
            self.assertTrue(any("category" in error and "miscomputed" in error for error in errors))
            self.assertTrue(any("unsafe path/secret" in error for error in errors))


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
