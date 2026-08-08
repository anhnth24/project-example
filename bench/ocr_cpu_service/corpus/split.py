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
_HOLDOUT_SOURCE_IDS = frozenset(
    {
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
    if annotation.split != deterministic_split(annotation.source_id):
        raise ValueError(f"line {line}: split disagrees with frozen source-family split")
    if annotation.provenance == "wikisource":
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
        if annotation.proofread_status == "Proofread" and annotation.reviewers:
            raise ValueError(
                f"line {line}: Proofread page cannot claim independent reviewers"
            )
        if annotation.proofread_status == "Validated" and not annotation.reviewers:
            raise ValueError(f"line {line}: Validated page needs reviewer evidence")
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
        key = (row.source_id, row.page_number)
        if key in seen:
            raise ValueError(
                f"line {line_number}: duplicate source/page {row.source_id}/{row.page_number}"
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
    args = parser.parse_args()
    rows = load_accuracy_annotations(args.annotations)
    evidence: dict[str, Any] = {
        "annotations": len(rows),
        "digest": annotation_digest(rows),
        "holdout": sum(row.split == "holdout" for row in rows),
        "tuning": sum(row.split == "tuning" for row in rows),
    }
    if args.assets:
        evidence.update(validate_local_assets(rows, args.assets))
    print(json.dumps(evidence, sort_keys=True))


if __name__ == "__main__":
    main()
