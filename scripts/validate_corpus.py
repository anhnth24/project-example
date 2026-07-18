#!/usr/bin/env python3
"""Validate the deterministic Phase 0 golden and adversarial corpus."""

from __future__ import annotations

import argparse
import csv
import datetime as dt
import hashlib
import json
import re
import shutil
import subprocess
import sys
import tempfile
import unicodedata
import unittest
from pathlib import Path, PurePosixPath

import blake3


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CORPUS = ROOT / "bench/markhand_web"
FORMATS = {
    "pdf_native",
    "pdf_scan",
    "docx",
    "pptx",
    "xlsx",
    "csv",
    "html",
    "image_ocr",
    "audio",
    "text_legacy",
}
THREATS = {
    "extension_spoof",
    "mime_mismatch",
    "parser_corruption",
    "prompt_injection",
    "csv_formula",
    "malformed_ooxml",
    "archive_path_traversal",
    "archive_bomb",
    "pdf_page_bomb",
    "audio_duration_limit",
}
DISPOSITIONS = {"reject", "quarantine"}
SHA256 = re.compile(r"^[0-9a-f]{64}$")
BLAKE3 = re.compile(r"^[0-9a-f]{64}$")
SECRET_PATTERNS = (
    re.compile(rb"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----"),
    re.compile(rb"\bAKIA[0-9A-Z]{16}\b"),
    re.compile(rb"\bpostgres(?:ql)?://[^/\s:@]+:[^@\s/]+@"),
)


def checksum(path: Path) -> dict[str, str]:
    data = path.read_bytes()
    return {
        "sha256": hashlib.sha256(data).hexdigest(),
        "blake3": blake3.blake3(data).hexdigest(),
    }


def safe_path(root: Path, raw: object) -> Path:
    if not isinstance(raw, str):
        raise ValueError("path must be a string")
    pure = PurePosixPath(raw)
    if pure.is_absolute() or ".." in pure.parts or raw != pure.as_posix():
        raise ValueError(f"path must be normalized and relative: {raw}")
    resolved = (root / pure).resolve()
    if not resolved.is_relative_to(root.resolve()):
        raise ValueError(f"path escapes corpus root: {raw}")
    return resolved


def load_json(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def timestamp(value: object) -> dt.datetime | None:
    if not isinstance(value, str) or not value.endswith("Z"):
        return None
    try:
        parsed = dt.datetime.fromisoformat(value[:-1] + "+00:00")
    except ValueError:
        return None
    return parsed if parsed.tzinfo is not None else None


def validate_checksum(path: Path, item: dict, label: str) -> list[str]:
    errors = []
    expected_sha = item.get("sha256")
    expected_blake = item.get("blake3")
    if not isinstance(expected_sha, str) or not SHA256.fullmatch(expected_sha):
        errors.append(f"{label}: invalid sha256")
    if not isinstance(expected_blake, str) or not BLAKE3.fullmatch(expected_blake):
        errors.append(f"{label}: invalid blake3")
    if path.is_file():
        actual = checksum(path)
        if actual["sha256"] != expected_sha:
            errors.append(f"{label}: sha256 mismatch")
        if actual["blake3"] != expected_blake:
            errors.append(f"{label}: blake3 mismatch")
        data = path.read_bytes()
        if any(pattern.search(data) for pattern in SECRET_PATTERNS):
            errors.append(f"{label}: secret canary detected")
    else:
        errors.append(f"{label}: file missing")
    return errors


def validate(root: Path, require_adjudicated: bool = True) -> list[str]:
    golden = root / "golden"
    adversarial = root / "adversarial"
    errors: list[str] = []
    try:
        golden_manifest = load_json(golden / "manifest.json")
        adversarial_manifest = load_json(adversarial / "manifest.json")
        lock = load_json(root / "manifest.lock.json")
    except (OSError, json.JSONDecodeError) as error:
        return [f"corpus manifest unreadable: {error}"]

    documents: dict[str, dict] = {}
    versions: dict[str, dict] = {}
    logical_versions: dict[str, list[dict]] = {}
    formats: set[str] = set()
    managed: set[str] = set()
    dependencies = golden_manifest.get("dependencies", [])
    dependency_ids = {
        dependency.get("id")
        for dependency in dependencies
        if isinstance(dependency, dict)
    }
    if "dejavu-sans" not in dependency_ids:
        errors.append("golden: DejaVu font dependency is missing")
    for dependency in dependencies:
        evidence = dependency.get("evidence")
        try:
            evidence_path = safe_path(ROOT, evidence)
        except ValueError as error:
            errors.append(f"golden dependency: {error}")
            continue
        if not evidence_path.is_file():
            errors.append(f"golden dependency evidence missing: {evidence}")
        elif hashlib.sha256(evidence_path.read_bytes()).hexdigest() != dependency.get(
            "evidenceSha256"
        ):
            errors.append("golden dependency: evidence checksum differs")
        if dependency.get("license") != "Bitstream-Vera":
            errors.append("golden dependency: unexpected font license")
        font = Path("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf")
        if font.is_file() and checksum(font)["sha256"] != dependency.get("sha256"):
            errors.append("golden dependency: system font checksum differs")
    for item in golden_manifest.get("documents", []):
        item_id = item.get("id")
        label = f"golden {item_id}"
        if not isinstance(item_id, str) or not item_id or item_id in documents:
            errors.append(f"{label}: duplicate or missing id")
            continue
        documents[item_id] = item
        version_id = item.get("versionId")
        logical_id = item.get("logicalDocumentId")
        version_number = item.get("versionNumber")
        if (
            not isinstance(version_id, str)
            or not version_id
            or version_id in versions
            or not isinstance(logical_id, str)
            or not logical_id
            or not isinstance(version_number, int)
            or isinstance(version_number, bool)
            or version_number < 1
            or timestamp(item.get("effectiveAt")) is None
            or not isinstance(item.get("isCurrent"), bool)
            or not isinstance(item.get("changeSummary"), str)
            or not item["changeSummary"].strip()
        ):
            errors.append(f"{label}: invalid document version metadata")
        else:
            versions[version_id] = item
            logical_versions.setdefault(logical_id, []).append(item)
        format_name = item.get("format")
        if format_name not in FORMATS:
            errors.append(f"{label}: unsupported format")
        else:
            formats.add(format_name)
        if item.get("source") != "generated" or item.get("license") != "CC0-1.0":
            errors.append(f"{label}: source/license must be generated/CC0-1.0")
        if not isinstance(item.get("owner"), str) or not item["owner"].strip():
            errors.append(f"{label}: owner must be non-empty")
        if any(dependency not in dependency_ids for dependency in item.get("dependencies", [])):
            errors.append(f"{label}: unknown dependency")
        if item.get("sensitive") is not False:
            errors.append(f"{label}: sensitive must be false")
        try:
            artifact = safe_path(golden, item.get("path"))
            markdown_path = safe_path(golden, item.get("markdownPath"))
        except ValueError as error:
            errors.append(f"{label}: {error}")
            continue
        managed.update(
            {
                artifact.relative_to(root).as_posix(),
                markdown_path.relative_to(root).as_posix(),
            }
        )
        errors.extend(validate_checksum(artifact, item, label))
        if not markdown_path.is_file():
            errors.append(f"{label}: canonical markdown missing")
        else:
            markdown = markdown_path.read_text(encoding="utf-8")
            if unicodedata.normalize("NFC", markdown) != markdown:
                errors.append(f"{label}: canonical markdown is not NFC")
            actual = hashlib.sha256(markdown.encode()).hexdigest()
            if actual != item.get("markdownSha256"):
                errors.append(f"{label}: markdownSha256 mismatch")
            if format_name == "audio":
                if markdown or item.get("expectedBehavior") != "empty_transcript":
                    errors.append(f"{label}: tone audio must expect an empty transcript")
            elif not markdown or item.get("expectedBehavior") != "content_preserved":
                errors.append(f"{label}: content fixture requires canonical markdown")
    missing_formats = FORMATS - formats
    if missing_formats:
        errors.append(f"golden: missing formats {sorted(missing_formats)}")
    for logical_id, items in logical_versions.items():
        numbers = [item["versionNumber"] for item in items]
        if len(numbers) != len(set(numbers)):
            errors.append(f"logical document {logical_id}: duplicate version number")
        if sum(item["isCurrent"] for item in items) != 1:
            errors.append(f"logical document {logical_id}: requires exactly one current version")
        ordered = sorted(items, key=lambda item: item["versionNumber"])
        for index, item in enumerate(ordered):
            expected_parent = None if index == 0 else ordered[index - 1]["versionId"]
            if item.get("parentVersionId") != expected_parent:
                errors.append(f"{item['id']}: parentVersionId breaks version lineage")
        effective = [timestamp(item["effectiveAt"]) for item in ordered]
        if effective != sorted(effective):
            errors.append(f"logical document {logical_id}: effectiveAt is not monotonic")

    query_path = golden / "queries.tsv"
    managed.add(query_path.relative_to(root).as_posix())
    query_ids: set[str] = set()
    query_rows_by_id: dict[str, dict] = {}
    categories: set[str] = set()
    query_count = 0
    no_answer_count = 0
    relevance_grades: set[int] = set()
    try:
        with query_path.open(encoding="utf-8", newline="") as source:
            for row in csv.DictReader(source, delimiter="\t"):
                query_count += 1
                query_id = row.get("query_id", "")
                if not query_id or query_id in query_ids:
                    errors.append(f"query {query_id}: duplicate or missing id")
                query_ids.add(query_id)
                query_rows_by_id[query_id] = row
                query = row.get("query", "")
                if not query or unicodedata.normalize("NFC", query) != query:
                    errors.append(f"query {query_id}: query must be non-empty NFC")
                categories.add(row.get("category", ""))
                try:
                    judgments = json.loads(row.get("judgments", ""))
                except json.JSONDecodeError:
                    errors.append(f"query {query_id}: judgments must be JSON")
                    judgments = {}
                if not isinstance(judgments, dict):
                    errors.append(f"query {query_id}: judgments must be an object")
                    judgments = {}
                for judged_doc, grade in judgments.items():
                    if judged_doc not in documents:
                        errors.append(f"query {query_id}: judgment document missing")
                    if not isinstance(grade, int) or isinstance(grade, bool) or grade not in range(4):
                        errors.append(f"query {query_id}: judgment grade must be 0..3")
                    else:
                        relevance_grades.add(grade)
                try:
                    citations = json.loads(row.get("citations", ""))
                    version_context = json.loads(row.get("version_context", ""))
                except json.JSONDecodeError:
                    errors.append(f"query {query_id}: citations/version_context must be JSON")
                    citations = []
                    version_context = {}
                if not isinstance(citations, list) or not isinstance(version_context, dict):
                    errors.append(f"query {query_id}: invalid citations/version_context shape")
                    citations = []
                    version_context = {}
                cited_versions: list[str] = []
                for index, anchor in enumerate(citations):
                    if not isinstance(anchor, dict):
                        errors.append(f"query {query_id}: citation must be an object")
                        continue
                    if anchor.get("citationId") != f"CITE-{index + 1:04}":
                        errors.append(f"query {query_id}: citation IDs must be ordered")
                    cited_item = documents.get(anchor.get("documentId"))
                    if cited_item is None:
                        errors.append(f"query {query_id}: citation document missing")
                        continue
                    if judgments.get(cited_item["id"], 0) < 1:
                        errors.append(f"query {query_id}: citation document is not relevant")
                    for field in (
                        "logicalDocumentId",
                        "versionId",
                        "versionNumber",
                        "isCurrent",
                        "effectiveAt",
                    ):
                        if anchor.get(field) != cited_item.get(field):
                            errors.append(f"query {query_id}: citation {field} mismatch")
                    if anchor.get("contentSha256") != cited_item.get("markdownSha256"):
                        errors.append(f"query {query_id}: citation content hash mismatch")
                    if anchor.get("chunkId") is not None:
                        errors.append(f"query {query_id}: chunkId must remain null before P0-06")
                    markdown_bytes = safe_path(
                        golden, cited_item["markdownPath"]
                    ).read_bytes()
                    start = anchor.get("start")
                    end = anchor.get("end")
                    if (
                        not isinstance(start, int)
                        or isinstance(start, bool)
                        or not isinstance(end, int)
                        or isinstance(end, bool)
                        or not 0 <= start < end <= len(markdown_bytes)
                    ):
                        errors.append(f"query {query_id}: citation span is invalid")
                        continue
                    try:
                        quote = markdown_bytes[start:end].decode("utf-8")
                    except UnicodeDecodeError:
                        errors.append(f"query {query_id}: citation splits UTF-8")
                        continue
                    if quote != anchor.get("quote"):
                        errors.append(f"query {query_id}: citation quote mismatch")
                    expected_page = 1 if cited_item["format"].startswith("pdf") else None
                    if anchor.get("page") != expected_page:
                        errors.append(f"query {query_id}: citation page mismatch")
                    cited_versions.append(cited_item["versionId"])
                if row.get("answer_mode") == "no_answer":
                    no_answer_count += 1
                    if (
                        row.get("expected_doc")
                        or row.get("answer_text")
                        or row.get("expected_answer")
                        or judgments
                        or citations
                    ):
                        errors.append(f"query {query_id}: no-answer row has expected content")
                    continue
                document_id = row.get("expected_doc", "")
                item = documents.get(document_id)
                if item is None:
                    errors.append(f"query {query_id}: expected document missing")
                    continue
                try:
                    relevance = int(row.get("relevance", ""))
                except ValueError:
                    relevance = -1
                if judgments.get(document_id) != relevance:
                    errors.append(f"query {query_id}: expected relevance differs from judgments")
                if not row.get("expected_answer") or not citations:
                    errors.append(f"query {query_id}: answer and citations are required")
                context_versions = version_context.get("citedVersionIds", [])
                if context_versions != list(dict.fromkeys(cited_versions)):
                    errors.append(f"query {query_id}: version context differs from citations")
                version_mode = row.get("version_mode")
                if version_mode not in {"current", "as_of", "compare", "history"}:
                    errors.append(f"query {query_id}: invalid version mode")
                query_time = row.get("query_time", "")
                query_instant = timestamp(query_time)
                if query_instant is None:
                    errors.append(f"query {query_id}: query_time must be RFC3339 UTC")
                if version_mode == "current":
                    if any(
                        not versions[version_id]["isCurrent"]
                        or (
                            query_instant is not None
                            and timestamp(versions[version_id]["effectiveAt"])
                            > query_instant
                        )
                        for version_id in cited_versions
                        if version_id in versions
                    ):
                        errors.append(
                            f"query {query_id}: current answer cites historical/future version"
                        )
                if version_mode == "as_of":
                    as_of = row.get("as_of", "")
                    as_of_instant = timestamp(as_of)
                    expected_version = item.get("versionId")
                    eligible = sorted(
                        (
                            version
                            for version in logical_versions.get(
                                item.get("logicalDocumentId"), []
                            )
                            if as_of_instant is not None
                            and timestamp(version["effectiveAt"]) <= as_of_instant
                        ),
                        key=lambda version: timestamp(version["effectiveAt"]),
                    )
                    if (
                        as_of_instant is None
                        or not eligible
                        or eligible[-1]["versionId"] != expected_version
                    ):
                        errors.append(f"query {query_id}: as_of does not resolve expected version")
                    if any(
                        (
                            as_of_instant is not None
                            and timestamp(versions[version_id]["effectiveAt"])
                            > as_of_instant
                        )
                        or versions[version_id]["logicalDocumentId"]
                        != item.get("logicalDocumentId")
                        for version_id in cited_versions
                        if version_id in versions
                    ):
                        errors.append(f"query {query_id}: as_of cites future/unrelated version")
                if version_mode in {"compare", "history"}:
                    logical_ids = {
                        versions[version_id]["logicalDocumentId"]
                        for version_id in cited_versions
                        if version_id in versions
                    }
                    if len(set(cited_versions)) < 2 or len(logical_ids) != 1:
                        errors.append(f"query {query_id}: comparison requires one version lineage")
                if row.get("answer_mode") != "document_list":
                    if (
                        version_context.get("logicalDocumentId")
                        != item.get("logicalDocumentId")
                    ):
                        errors.append(
                            f"query {query_id}: version context logical document mismatch"
                        )
                    current_version = next(
                        (
                            version["versionId"]
                            for version in logical_versions.get(
                                item["logicalDocumentId"], []
                            )
                            if version["isCurrent"]
                        ),
                        None,
                    )
                    if version_context.get("currentVersionId") != current_version:
                        errors.append(f"query {query_id}: currentVersionId is incorrect")
                    expected_change_note = (
                        versions[current_version]["changeSummary"]
                        if item.get("versionFixture")
                        and version_mode in {"current", "compare", "history"}
                        and current_version in versions
                        else ""
                    )
                    if version_context.get("changeNote") != expected_change_note:
                        errors.append(f"query {query_id}: changeNote is incorrect")
                if row.get("answer_mode") == "document_list":
                    if row.get("span_start") or row.get("span_end") or row.get("answer_text"):
                        errors.append(f"query {query_id}: document-list row must not contain span")
                    expected_citation_docs = {
                        doc_id for doc_id, grade in judgments.items() if grade >= 2
                    }
                    actual_citation_docs = {
                        anchor.get("documentId") for anchor in citations
                    }
                    if actual_citation_docs != expected_citation_docs:
                        errors.append(
                            f"query {query_id}: document-list citations are incomplete"
                        )
                    continue
                if row.get("answer_mode") in {
                    "versioned_answer",
                    "version_compare",
                    "version_history",
                }:
                    if row.get("span_start") or row.get("span_end") or row.get("answer_text"):
                        errors.append(f"query {query_id}: synthesized version answer must not contain span")
                    expected_history = [
                        {
                            "versionId": version["versionId"],
                            "versionNumber": version["versionNumber"],
                            "effectiveAt": version["effectiveAt"],
                            "isCurrent": version["isCurrent"],
                            "changeSummary": version["changeSummary"],
                        }
                        for version in sorted(
                            logical_versions.get(item["logicalDocumentId"], []),
                            key=lambda version: version["versionNumber"],
                        )
                    ]
                    if version_context.get("history") != expected_history:
                        errors.append(f"query {query_id}: version history context is incomplete")
                    continue
                markdown_path = safe_path(golden, item["markdownPath"])
                markdown_bytes = markdown_path.read_bytes()
                try:
                    start = int(row.get("span_start", ""))
                    end = int(row.get("span_end", ""))
                except ValueError:
                    errors.append(f"query {query_id}: span is not numeric")
                    continue
                if not 0 <= start < end <= len(markdown_bytes):
                    errors.append(f"query {query_id}: span is outside markdown")
                    continue
                try:
                    answer = markdown_bytes[start:end].decode("utf-8")
                except UnicodeDecodeError:
                    errors.append(f"query {query_id}: span splits UTF-8")
                    continue
                if answer != row.get("answer_text"):
                    errors.append(f"query {query_id}: span does not match answer text")
                if len(citations) != 1 or citations[0].get("quote") != answer:
                    errors.append(f"query {query_id}: direct answer citation mismatch")
                if row.get("expected_answer") != answer:
                    errors.append(f"query {query_id}: direct expected answer mismatch")
                if row.get("relevance") not in {"1", "2", "3"}:
                    errors.append(f"query {query_id}: invalid relevance")
                if item["format"].startswith("pdf") and row.get("page") != "1":
                    errors.append(f"query {query_id}: PDF answer requires page 1")
    except OSError as error:
        errors.append(f"queries.tsv unreadable: {error}")
    if not 200 <= query_count <= 500:
        errors.append(f"queries: expected 200..500 rows, got {query_count}")
    if no_answer_count < 25:
        errors.append("queries: fewer than 25 no-answer cases")
    if not {1, 2, 3}.issubset(relevance_grades):
        errors.append("queries: graded judgments must cover relevance 1, 2 and 3")
    for category in (
        "named_entity",
        "diacritic_variant",
        "table_numeric",
        "numeric_fact",
        "long_context",
        "abbreviation",
        "multi_doc",
        "temporal_current",
        "temporal_as_of",
        "version_compare",
        "version_history",
        "no_answer",
        "prompt_injection_query",
    ):
        if category not in categories:
            errors.append(f"queries: missing category {category}")

    adjudication_path = golden / "adjudication.json"
    review_sample_path = golden / "review-sample.tsv"
    managed.update(
        {
            adjudication_path.relative_to(root).as_posix(),
            review_sample_path.relative_to(root).as_posix(),
        }
    )
    try:
        adjudication = load_json(adjudication_path)
    except (OSError, json.JSONDecodeError) as error:
        errors.append(f"adjudication unreadable: {error}")
        adjudication = {}
    sample_ids = adjudication.get("sampleQueryIds", [])
    if (
        adjudication.get("version") != 1
        or not isinstance(sample_ids, list)
        or len(sample_ids) < 50
        or len(set(sample_ids)) != len(sample_ids)
        or not set(sample_ids).issubset(query_ids)
    ):
        errors.append("adjudication requires at least 50 unique known sample queries")
    if (
        review_sample_path.is_file()
        and adjudication.get("sampleSha256")
        != hashlib.sha256(review_sample_path.read_bytes()).hexdigest()
    ):
        errors.append("adjudication sampleSha256 does not match review packet")
    reviews = adjudication.get("reviews", [])
    approved_roles = {
        review.get("role")
        for review in reviews
        if isinstance(review, dict)
        and review.get("decision") == "approved"
        and isinstance(review.get("reviewer"), str)
        and review["reviewer"].strip()
        and isinstance(review.get("reviewedAt"), str)
        and review["reviewedAt"].strip()
    }
    required_roles = set(adjudication.get("requiredRoles", []))
    if require_adjudicated and (
        adjudication.get("status") != "approved"
        or len(approved_roles) < 2
        or not required_roles.issubset(approved_roles)
    ):
        errors.append("adjudication requires approved domain and retrieval reviews")
    if not review_sample_path.is_file():
        errors.append("review-sample.tsv missing")
    else:
        with review_sample_path.open(encoding="utf-8", newline="") as source:
            review_rows = list(csv.DictReader(source, delimiter="\t"))
        if len(review_rows) != 50:
            errors.append("review-sample.tsv must contain exactly 50 rows")
        for row in review_rows:
            if query_rows_by_id.get(row.get("query_id", "")) != row:
                errors.append(
                    f"review sample diverges from queries.tsv: {row.get('query_id', '')}"
                )

    attack_ids: set[str] = set()
    threats: set[str] = set()
    for item in adversarial_manifest.get("attacks", []):
        item_id = item.get("id")
        label = f"adversarial {item_id}"
        if not isinstance(item_id, str) or not item_id or item_id in attack_ids:
            errors.append(f"{label}: duplicate or missing id")
            continue
        attack_ids.add(item_id)
        threat = item.get("threatClass")
        if threat not in THREATS:
            errors.append(f"{label}: invalid threat class")
        else:
            threats.add(threat)
        if item.get("expectedDisposition") not in DISPOSITIONS:
            errors.append(f"{label}: invalid expected disposition")
        if item.get("source") != "generated" or item.get("license") != "CC0-1.0":
            errors.append(f"{label}: source/license must be generated/CC0-1.0")
        if not isinstance(item.get("owner"), str) or not item["owner"].strip():
            errors.append(f"{label}: owner must be non-empty")
        if item.get("sensitive") is not False:
            errors.append(f"{label}: sensitive must be false")
        try:
            path = safe_path(adversarial, item.get("path"))
        except ValueError as error:
            errors.append(f"{label}: {error}")
            continue
        managed.add(path.relative_to(root).as_posix())
        errors.extend(validate_checksum(path, item, label))
    missing_threats = THREATS - threats
    if missing_threats:
        errors.append(f"adversarial: missing threats {sorted(missing_threats)}")

    managed.update({"golden/manifest.json", "adversarial/manifest.json"})
    lock_entries = lock.get("files", [])
    lock_paths: set[str] = set()
    for item in lock_entries:
        raw = item.get("path")
        if not isinstance(raw, str) or raw in lock_paths:
            errors.append(f"lock: duplicate or invalid path {raw}")
            continue
        lock_paths.add(raw)
        try:
            path = safe_path(root, raw)
        except ValueError as error:
            errors.append(f"lock: {error}")
            continue
        errors.extend(validate_checksum(path, item, f"lock {raw}"))
    if lock_paths != managed:
        errors.append(
            f"lock: path set differs; missing={sorted(managed-lock_paths)}, extra={sorted(lock_paths-managed)}"
        )
    actual_managed = {
        path.relative_to(root).as_posix()
        for base in (golden, adversarial)
        for path in base.rglob("*")
        if path.is_file()
    }
    if actual_managed != managed:
        errors.append(
            f"corpus: unmanaged files {sorted(actual_managed-managed)}"
        )
    return errors


def reproducibility_errors(root: Path) -> list[str]:
    with tempfile.TemporaryDirectory() as temporary:
        output = Path(temporary) / "markhand_web"
        command = [
            sys.executable,
            str(root / "scripts/generate_corpus.py"),
            "--output",
            str(output),
        ]
        completed = subprocess.run(command, capture_output=True, text=True, check=False)
        if completed.returncode != 0:
            return [f"reproducibility generator failed: {completed.stderr}"]
        expected = load_json(root / "manifest.lock.json")
        actual = load_json(output / "manifest.lock.json")
        excluded = {"golden/adjudication.json"}
        expected_files = {
            item["path"]: item
            for item in expected.get("files", [])
            if item.get("path") not in excluded
        }
        actual_files = {
            item["path"]: item
            for item in actual.get("files", [])
            if item.get("path") not in excluded
        }
        return (
            []
            if expected_files == actual_files
            else ["reproducibility lock differs after regeneration"]
        )


class CorpusValidatorTests(unittest.TestCase):
    def test_safe_path_rejects_absolute_and_traversal(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with self.assertRaises(ValueError):
                safe_path(root, "../escape")
            with self.assertRaises(ValueError):
                safe_path(root, "/absolute")

    def test_checksum_detects_mutation(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "sample.txt"
            path.write_text("original")
            item = checksum(path)
            path.write_text("changed")
            errors = validate_checksum(path, item, "sample")
            self.assertTrue(any("mismatch" in error for error in errors))

    def test_rejects_temporal_and_multi_citation_corruption(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "markhand_web"
            shutil.copytree(DEFAULT_CORPUS, root)
            query_path = root / "golden/queries.tsv"
            with query_path.open(encoding="utf-8", newline="") as source:
                rows = list(csv.DictReader(source, delimiter="\t"))
                fields = list(rows[0])
            current_rows = [
                row for row in rows if row["category"] == "temporal_current"
            ]
            current = current_rows[0]
            as_of = next(row for row in rows if row["version_mode"] == "as_of")
            future_citation = json.loads(current["citations"])[0]
            as_of["citations"] = json.dumps([future_citation], separators=(",", ":"))
            as_of_context = json.loads(as_of["version_context"])
            as_of_context["citedVersionIds"] = [future_citation["versionId"]]
            as_of["version_context"] = json.dumps(as_of_context, separators=(",", ":"))
            document_list = next(row for row in rows if row["answer_mode"] == "document_list")
            document_list["citations"] = json.dumps(
                json.loads(document_list["citations"])[:-1],
                separators=(",", ":"),
            )
            compare = next(row for row in rows if row["version_mode"] == "compare")
            compare_context = json.loads(compare["version_context"])
            compare_context["history"] = []
            compare_context["logicalDocumentId"] = "wrong-lineage"
            compare_context["changeNote"] = "wrong change"
            compare["version_context"] = json.dumps(compare_context, separators=(",", ":"))
            current["query_time"] = "zzzz"
            current_rows[1]["query_time"] = "2026-01-01T00:00:00Z"
            with query_path.open("w", encoding="utf-8", newline="") as output:
                writer = csv.DictWriter(
                    output, fields, delimiter="\t", lineterminator="\n"
                )
                writer.writeheader()
                writer.writerows(rows)
            errors = validate(root, require_adjudicated=False)
            self.assertTrue(any("as_of cites future" in error for error in errors))
            self.assertTrue(any("document-list citations are incomplete" in error for error in errors))
            self.assertTrue(any("version history context is incomplete" in error for error in errors))
            self.assertTrue(any("query_time must be RFC3339" in error for error in errors))
            self.assertTrue(any("current answer cites historical/future" in error for error in errors))
            self.assertTrue(any("logical document mismatch" in error for error in errors))
            self.assertTrue(any("changeNote is incorrect" in error for error in errors))
            self.assertTrue(any("review sample diverges" in error for error in errors))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=DEFAULT_CORPUS)
    parser.add_argument("--reproducible", action="store_true")
    parser.add_argument("--allow-pending", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(CorpusValidatorTests)
        return 0 if unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful() else 1
    root = args.root.resolve()
    errors = validate(root, require_adjudicated=not args.allow_pending)
    if args.reproducible:
        errors.extend(reproducibility_errors(root))
    if errors:
        print("corpus validation failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    print("Phase 0 corpus validation passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
