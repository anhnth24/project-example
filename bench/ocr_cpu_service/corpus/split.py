"""Strict schema and source-family split validation for the frozen OCR corpus."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any, Iterable

_FIELDS = (
    "page_id",
    "split",
    "source_id",
    "source_family",
    "page_number",
    "asset_path",
    "image_sha256",
    "source_sha256",
    "transcription",
    "transcription_origin",
    "review_status",
    "proofread_status",
    "page_url",
    "revision_id",
    "contributors",
    "reviewers",
    "license",
    "dataset_license",
    "document_type",
    "difficulty_strata",
    "provenance",
)
_SHA256 = re.compile(r"[0-9a-f]{64}")
_PAGE_QUALITY = re.compile(r'<pagequality level="([0-4])" user="([^"]*)"')
_DIFFICULTY_STRATA = frozenset(
    {
        "clean-official",
        "dense-form",
        "dense-text",
        "historical-old-print",
        "low-contrast",
        "modern-government",
        "multi-column",
        "skew",
        "small-text",
        "stamp-watermark",
    }
)
_WIKISOURCE_LICENSES = ("CC-BY-SA-4.0", "CC-BY-NC-SA-4.0")
_NRL_LICENSES = (
    "Public Domain (Luật SHTT VN, Điều 15)",
    "CC0-1.0",
)
_CORRECTED_VIETAGE_PAGES = frozenset(
    {
        "wikisource:Cung oan ngam khuc 1905.pdf:0015",
        "wikisource:Cung oan ngam khuc 1905.pdf:0016",
    }
)
_HOLDOUT_SOURCE_IDS = frozenset(
    {
        "Cung oan ngam khuc 1905.pdf",
        "Kinh Thanh Cuu Uoc Va Tan Uoc 1925.pdf",
        "Tan Da tung van.pdf",
        "cung-oan-ngam-khuc-1905-pdf",
        "kinh-thanh-cuu-uoc-va-tan-uoc-1925-pdf",
        "tan-da-tung-van-pdf",
    }
)


@dataclass(frozen=True, slots=True)
class AccuracyAnnotation:
    page_id: str
    split: str
    source_id: str
    source_family: str
    page_number: int
    asset_path: str
    image_sha256: str
    source_sha256: str
    transcription: str
    transcription_origin: str
    review_status: str
    proofread_status: str
    page_url: str
    revision_id: int
    contributors: tuple[str, ...]
    reviewers: tuple[str, ...]
    license: str
    dataset_license: str
    document_type: str
    difficulty_strata: tuple[str, ...]
    provenance: str


def deterministic_split(source_id: str) -> str:
    """Assign the three frozen validated families to holdout, all others to tuning."""
    return "holdout" if source_id in _HOLDOUT_SOURCE_IDS else "tuning"


def _required_text(row: dict[str, Any], field: str, line: int) -> str:
    value = row[field]
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"line {line}: {field} is required")
    return value


def _string_tuple(row: dict[str, Any], field: str, line: int) -> tuple[str, ...]:
    value = row[field]
    if not isinstance(value, list) or any(
        not isinstance(item, str) or not item.strip() for item in value
    ):
        raise ValueError(f"line {line}: {field} must be a list of non-empty strings")
    return tuple(value)


def _annotation(row: Any, line: int) -> AccuracyAnnotation:
    if not isinstance(row, dict):
        raise ValueError(f"line {line}: annotation must be an object")
    missing = [field for field in _FIELDS if field not in row]
    if missing:
        raise ValueError(f"line {line}: missing required field {missing[0]}")
    unexpected = sorted(set(row) - set(_FIELDS))
    if unexpected:
        raise ValueError(f"line {line}: unexpected fields: {', '.join(unexpected)}")

    text = {field: _required_text(row, field, line) for field in _FIELDS if field not in {
        "page_number",
        "revision_id",
        "contributors",
        "reviewers",
        "difficulty_strata",
    }}
    if row["split"] not in {"tuning", "holdout"}:
        raise ValueError(f"line {line}: split must be tuning or holdout")
    if not isinstance(row["page_number"], int) or isinstance(row["page_number"], bool):
        raise ValueError(f"line {line}: page_number must be an integer")
    if row["page_number"] <= 0:
        raise ValueError(f"line {line}: page_number must be positive")
    if not isinstance(row["revision_id"], int) or isinstance(row["revision_id"], bool):
        raise ValueError(f"line {line}: revision_id must be an integer")
    if row["revision_id"] < 0:
        raise ValueError(f"line {line}: revision_id cannot be negative")
    for field in ("image_sha256", "source_sha256"):
        if not _SHA256.fullmatch(row[field]):
            raise ValueError(f"line {line}: {field} must be lowercase SHA-256")

    annotation = AccuracyAnnotation(
        **text,
        page_number=row["page_number"],
        revision_id=row["revision_id"],
        contributors=_string_tuple(row, "contributors", line),
        reviewers=_string_tuple(row, "reviewers", line),
        difficulty_strata=_string_tuple(row, "difficulty_strata", line),
    )
    if annotation.review_status != "human-verified":
        raise ValueError(f"line {line}: review_status must be human-verified")
    if (
        not annotation.difficulty_strata
        or not set(annotation.difficulty_strata) <= _DIFFICULTY_STRATA
    ):
        raise ValueError(f"line {line}: difficulty_strata contains an unknown value")
    if annotation.split != deterministic_split(annotation.source_family):
        raise ValueError(f"line {line}: split disagrees with frozen source-family split")
    if annotation.provenance == "wikisource":
        if (annotation.license, annotation.dataset_license) != _WIKISOURCE_LICENSES:
            raise ValueError(f"line {line}: Wikisource licenses are not allowed")
        if annotation.source_id != "vietage-ocr-test":
            raise ValueError(f"line {line}: Wikisource source_id is not pinned")
        if annotation.transcription_origin != "wikisource-proofreadpage":
            raise ValueError(f"line {line}: Wikisource transcription origin is invalid")
        if annotation.proofread_status not in {"Proofread", "Validated"}:
            raise ValueError(f"line {line}: Wikisource status is not admissible")
        if not annotation.page_url.startswith(
            "https://vi.wikisource.org/wiki/Trang:"
        ):
            raise ValueError(f"line {line}: direct Wikisource Page URL is required")
        if annotation.revision_id <= 0:
            raise ValueError(f"line {line}: exact Wikisource revision is required")
        if not annotation.contributors:
            raise ValueError(f"line {line}: Wikisource contributor evidence is required")
        if len(annotation.contributors) != 1:
            raise ValueError(f"line {line}: Wikisource needs exactly one proofreader")
        if annotation.proofread_status == "Proofread":
            if annotation.reviewers:
                raise ValueError(
                    f"line {line}: Proofread page cannot claim independent reviewers"
                )
        elif len(annotation.reviewers) != 1:
            raise ValueError(f"line {line}: Validated page needs exactly one validator")
        elif annotation.reviewers[0] == annotation.contributors[0]:
            raise ValueError(f"line {line}: validator must be distinct from proofreader")
    elif annotation.provenance == "nrl-ai":
        if (annotation.license, annotation.dataset_license) != _NRL_LICENSES:
            raise ValueError(f"line {line}: nrl-ai licenses are not allowed")
        if annotation.transcription_origin != "dataset-declared-human-verified":
            raise ValueError(f"line {line}: nrl-ai transcription origin is invalid")
        if len(annotation.contributors) != 1 or annotation.reviewers:
            raise ValueError(f"line {line}: nrl-ai review evidence is invalid")
    else:
        raise ValueError(f"line {line}: provenance is not recognized")
    return annotation


def load_accuracy_annotations(path: Path) -> list[AccuracyAnnotation]:
    """Load strict JSONL and fail closed on malformed or duplicate pages."""
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        raise ValueError(f"{path}: cannot read annotations: {error}") from error
    rows: list[AccuracyAnnotation] = []
    seen: set[tuple[str, int]] = set()
    for line_number, line in enumerate(lines, 1):
        if not line.strip():
            raise ValueError(f"line {line_number}: blank lines are not allowed")
        try:
            raw = json.loads(line)
        except json.JSONDecodeError as error:
            raise ValueError(f"line {line_number}: invalid JSON: {error}") from error
        row = _annotation(raw, line_number)
        key = (row.source_family, row.page_number)
        if key in seen:
            raise ValueError(
                f"line {line_number}: duplicate source/page "
                f"{row.source_family}/{row.page_number}"
            )
        seen.add(key)
        rows.append(row)
    validate_accuracy_corpus(rows)
    return rows


def no_source_overlap(rows: Iterable[AccuracyAnnotation]) -> bool:
    assignments: dict[str, set[str]] = {}
    for row in rows:
        assignments.setdefault(row.source_family, set()).add(row.split)
    return all(len(splits) == 1 for splits in assignments.values())


def validate_accuracy_corpus(rows: Iterable[AccuracyAnnotation]) -> None:
    if not no_source_overlap(rows):
        raise ValueError("source family crosses splits")


def load_accuracy_provenance(path: Path) -> dict[str, Any]:
    try:
        audit = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"{path}: invalid provenance audit: {error}") from error
    if not isinstance(audit, dict):
        raise ValueError(f"{path}: provenance audit must be an object")
    if set(audit) != {"schema_version", "sources_manifest_sha256", "records"}:
        raise ValueError(f"{path}: provenance audit fields are invalid")
    if audit["schema_version"] != 1 or not isinstance(audit["records"], list):
        raise ValueError(f"{path}: unsupported provenance audit schema")
    if not isinstance(audit["sources_manifest_sha256"], str) or not _SHA256.fullmatch(
        audit["sources_manifest_sha256"]
    ):
        raise ValueError(f"{path}: source manifest checksum is invalid")
    return audit


def _text_sha256(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


def _canonical_sha256(value: Any) -> str:
    serialized = json.dumps(
        value, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    )
    return _text_sha256(serialized)


def _validate_quality_revision(
    revision: Any, expected_level: int, expected_user: str
) -> None:
    if not isinstance(revision, dict):
        raise ValueError("Wikisource quality revision must be an object")
    content = revision.get("content")
    if not isinstance(content, str):
        raise ValueError("Wikisource revision content is required")
    if revision.get("content_sha256") != _text_sha256(content):
        raise ValueError("Wikisource revision content checksum mismatch")
    match = _PAGE_QUALITY.search(content)
    if not match:
        raise ValueError("Wikisource revision lacks pagequality evidence")
    if int(match.group(1)) != expected_level or match.group(2) != expected_user:
        raise ValueError("Wikisource quality identity/status mismatch")


def validate_accuracy_provenance(
    rows: Iterable[AccuracyAnnotation],
    sources: Iterable[Any],
    audit: dict[str, Any],
) -> None:
    """Bind every annotation to pinned source and immutable offline evidence."""
    rows = list(rows)
    sources = list(sources)
    source_by_id = {source.id: source for source in sources}
    reconstructed_manifest = json.dumps(
        [asdict(source) for source in sources], ensure_ascii=False, indent=2
    ) + "\n"
    if audit["sources_manifest_sha256"] != _text_sha256(reconstructed_manifest):
        raise ValueError("source manifest checksum mismatch")
    records = audit["records"]
    if len(records) != len(rows):
        raise ValueError("provenance record count does not match annotations")
    record_by_page = {record.get("page_id"): record for record in records}
    if len(record_by_page) != len(records):
        raise ValueError("duplicate provenance page record")

    corrected_pages: set[str] = set()
    for row in rows:
        record = record_by_page.get(row.page_id)
        if not isinstance(record, dict):
            raise ValueError(f"missing provenance record: {row.page_id}")
        source = source_by_id.get(row.source_id)
        if source is None or source.sha256 != row.source_sha256:
            raise ValueError(f"annotation source is not pinned: {row.page_id}")
        if row.dataset_license != source.license:
            raise ValueError(f"annotation dataset license differs from source: {row.page_id}")
        if (
            record.get("source_id") != row.source_id
            or record.get("source_sha256") != row.source_sha256
            or record.get("image_sha256") != row.image_sha256
            or record.get("annotation_sha256") != _text_sha256(_canonical_json(row))
        ):
            raise ValueError(f"provenance annotation binding mismatch: {row.page_id}")
        unsigned_record = dict(record)
        record_sha256 = unsigned_record.pop("record_sha256", None)
        if record_sha256 != _canonical_sha256(unsigned_record):
            raise ValueError(f"provenance record checksum mismatch: {row.page_id}")
        evidence = record.get("evidence")
        if not isinstance(evidence, dict):
            raise ValueError(f"missing source evidence: {row.page_id}")

        if row.provenance == "wikisource":
            if evidence.get("kind") != "wikisource-proofreadpage":
                raise ValueError(f"wrong Wikisource evidence kind: {row.page_id}")
            current = evidence.get("current_revision")
            if not isinstance(current, dict) or current.get("revision_id") != row.revision_id:
                raise ValueError(f"Wikisource revision binding mismatch: {row.page_id}")
            expected_level = 4 if row.proofread_status == "Validated" else 3
            expected_quality_user = (
                row.reviewers[0]
                if row.proofread_status == "Validated"
                else row.contributors[0]
            )
            _validate_quality_revision(current, expected_level, expected_quality_user)
            quality_revisions = evidence.get("quality_revisions")
            expected_count = 2 if row.proofread_status == "Validated" else 1
            if not isinstance(quality_revisions, list) or len(quality_revisions) != expected_count:
                raise ValueError(f"Wikisource quality history is incomplete: {row.page_id}")
            _validate_quality_revision(quality_revisions[0], 3, row.contributors[0])
            if row.proofread_status == "Validated":
                _validate_quality_revision(quality_revisions[1], 4, row.reviewers[0])
            package_label = evidence.get("package_label")
            if (
                not isinstance(package_label, str)
                or evidence.get("package_label_sha256") != _text_sha256(package_label)
            ):
                raise ValueError(f"Wikisource package label checksum mismatch: {row.page_id}")
            transform = evidence.get("label_transform")
            if transform == "identity":
                expected_label = package_label
            elif transform == "remove-vietage-dongt-alignment-artifact-v1":
                corrected_pages.add(row.page_id)
                expected_label = re.sub(r"(?<=[.!?])r(?=\n)", "", package_label)
                if expected_label == package_label:
                    raise ValueError(f"Wikisource correction changed nothing: {row.page_id}")
            else:
                raise ValueError(f"unrecognized label transform: {row.page_id}")
            if row.transcription != expected_label:
                raise ValueError(f"Wikisource label evidence mismatch: {row.page_id}")
            if evidence.get("revision_api_url", "").find(f"revids={row.revision_id}") < 0:
                raise ValueError(f"Wikisource immutable API URL mismatch: {row.page_id}")
        else:
            if evidence.get("kind") != "nrl-ai-metadata":
                raise ValueError(f"wrong nrl-ai evidence kind: {row.page_id}")
            metadata = evidence.get("metadata_record")
            if not isinstance(metadata, dict) or evidence.get(
                "metadata_record_sha256"
            ) != _canonical_sha256(metadata):
                raise ValueError(f"nrl-ai metadata checksum mismatch: {row.page_id}")
            metadata_source = source_by_id.get(evidence.get("metadata_source_id"))
            if (
                metadata_source is None
                or metadata_source.sha256 != evidence.get("metadata_source_sha256")
            ):
                raise ValueError(f"nrl-ai metadata source is not pinned: {row.page_id}")
            if (
                metadata.get("doc_id") != row.source_family
                or metadata.get("text") != row.transcription
                or metadata.get("source_url") != row.page_url
                or metadata.get("license") != row.license
            ):
                raise ValueError(f"nrl-ai metadata binding mismatch: {row.page_id}")
    if corrected_pages != _CORRECTED_VIETAGE_PAGES:
        raise ValueError("Vietage label correction set is not exactly pinned")


def _canonical_json(row: AccuracyAnnotation) -> str:
    payload = asdict(row)
    payload["contributors"] = list(row.contributors)
    payload["reviewers"] = list(row.reviewers)
    payload["difficulty_strata"] = list(row.difficulty_strata)
    return json.dumps(
        payload, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    )


def annotation_digest(rows: Iterable[AccuracyAnnotation]) -> str:
    content = "".join(f"{_canonical_json(row)}\n" for row in rows)
    return hashlib.sha256(content.encode("utf-8")).hexdigest()


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def validate_local_assets(
    rows: Iterable[AccuracyAnnotation], asset_root: Path
) -> dict[str, int]:
    """Verify every selected local image exists once and matches its frozen checksum."""
    asset_count = 0
    total_bytes = 0
    seen_paths: set[str] = set()
    for row in rows:
        if row.asset_path in seen_paths:
            raise ValueError(f"duplicate local asset path: {row.asset_path}")
        seen_paths.add(row.asset_path)
        path = asset_root / row.asset_path
        if not path.is_file():
            raise ValueError(f"missing local asset: {row.asset_path}")
        if _sha256(path) != row.image_sha256:
            raise ValueError(f"local asset checksum mismatch: {row.asset_path}")
        asset_count += 1
        total_bytes += path.stat().st_size
    return {"asset_count": asset_count, "total_bytes": total_bytes}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("annotations", type=Path)
    parser.add_argument("--assets", type=Path)
    parser.add_argument("--sources", type=Path)
    parser.add_argument("--provenance", type=Path)
    args = parser.parse_args()
    if (args.sources is None) != (args.provenance is None):
        parser.error("--sources and --provenance must be supplied together")
    rows = load_accuracy_annotations(args.annotations)
    evidence: dict[str, Any] = {
        "annotations": len(rows),
        "digest": annotation_digest(rows),
        "holdout": sum(row.split == "holdout" for row in rows),
        "tuning": sum(row.split == "tuning" for row in rows),
    }
    if args.assets:
        evidence.update(validate_local_assets(rows, args.assets))
    if args.sources:
        from corpus.download import load_sources

        audit = load_accuracy_provenance(args.provenance)
        validate_accuracy_provenance(rows, load_sources(args.sources), audit)
        evidence["provenance_records"] = len(audit["records"])
    print(json.dumps(evidence, sort_keys=True))


if __name__ == "__main__":
    main()
