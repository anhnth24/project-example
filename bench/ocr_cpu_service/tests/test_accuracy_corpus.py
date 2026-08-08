from __future__ import annotations

import hashlib
import json
import shutil
import sys
from dataclasses import replace
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parents[1]))

from corpus.split import (  # noqa: E402
    annotation_digest,
    load_accuracy_annotations,
    no_source_overlap,
    validate_accuracy_corpus,
    validate_local_assets,
)
from corpus.download import load_sources  # noqa: E402


SERVICE_ROOT = Path(__file__).parents[1]
ANNOTATIONS = SERVICE_ROOT / "corpus" / "accuracy-annotations.jsonl"
SOURCES = SERVICE_ROOT / "corpus" / "accuracy-sources.json"


def test_frozen_corpus_has_exact_requested_composition() -> None:
    rows = load_accuracy_annotations(ANNOTATIONS)

    assert len(rows) == 50
    assert sum(row.split == "tuning" for row in rows) == 44
    assert sum(row.split == "holdout" for row in rows) == 6
    assert sum(
        row.split == "tuning" and row.provenance == "nrl-ai"
        for row in rows
    ) == 9
    assert sum(
        row.split == "tuning" and row.provenance == "wikisource"
        for row in rows
    ) == 35
    assert {
        row.source_family
        for row in rows
        if row.split == "tuning" and row.provenance == "wikisource"
    }.__len__() == 18
    assert {
        row.source_family for row in rows if row.split == "holdout"
    } == {
        "Cung oan ngam khuc 1905.pdf",
        "Kinh Thanh Cuu Uoc Va Tan Uoc 1925.pdf",
        "Tan Da tung van.pdf",
    }


def test_source_document_never_crosses_tuning_and_holdout() -> None:
    rows = load_accuracy_annotations(ANNOTATIONS)

    assert no_source_overlap(rows)


def test_real_pages_have_human_reference_license_and_checksums() -> None:
    rows = load_accuracy_annotations(ANNOTATIONS)

    for row in rows:
        assert row.review_status == "human-verified"
        assert row.transcription.strip()
        assert row.transcription_origin in {
            "dataset-declared-human-verified",
            "wikisource-proofreadpage",
        }
        assert row.license.strip()
        assert len(row.image_sha256) == 64
        assert len(row.source_sha256) == 64
        assert row.difficulty_strata
        assert row.document_type


def test_wikisource_rows_have_frozen_proofreadpage_evidence() -> None:
    rows = load_accuracy_annotations(ANNOTATIONS)
    wiki_rows = [row for row in rows if row.provenance == "wikisource"]

    assert len(wiki_rows) == 41
    for row in wiki_rows:
        assert row.page_url.startswith("https://vi.wikisource.org/wiki/Trang:")
        assert row.revision_id > 0
        assert row.proofread_status in {"Proofread", "Validated"}
        assert row.contributors
        assert row.license == "CC-BY-SA-4.0"
        assert row.dataset_license == "CC-BY-NC-SA-4.0"
        if row.proofread_status == "Proofread":
            assert not row.reviewers
        else:
            assert row.reviewers


def test_holdout_is_all_and_only_the_six_validated_pages() -> None:
    rows = load_accuracy_annotations(ANNOTATIONS)
    holdout = [row for row in rows if row.split == "holdout"]

    assert len(holdout) == 6
    assert all(row.proofread_status == "Validated" for row in holdout)
    assert all(row.provenance == "wikisource" for row in holdout)
    assert not {
        row.source_family for row in rows if row.split == "tuning"
    } & {
        "Cung oan ngam khuc 1905.pdf",
        "Kinh Thanh Cuu Uoc Va Tan Uoc 1925.pdf",
        "Tan Da tung van.pdf",
    }


def test_forbidden_sources_and_unreviewed_labels_are_absent() -> None:
    rows = load_accuracy_annotations(ANNOTATIONS)
    serialized = "\n".join(
        value
        for row in rows
        for value in (
            row.page_id,
            row.source_family,
            row.transcription_origin,
        )
    ).lower()

    assert "meddiesocr" not in serialized
    assert "pntv" not in serialized
    assert "ocr-extracted" not in serialized
    assert "pdf-extracted" not in serialized


def test_accuracy_source_manifest_is_frozen_and_license_explicit() -> None:
    sources = load_sources(SOURCES)
    package = next(source for source in sources if source.id == "vietage-ocr-test")

    assert package.sha256 == (
        "7909cfa665a890440eae791900c62f81629d38615fea2192913736b2ce491ea7"
    )
    assert "/4e22d5ff4cc0dce8c1128e8106852053f7192083/" in package.url
    assert package.license == "CC-BY-NC-SA-4.0"
    assert len([source for source in sources if source.kind == "real-scan"]) == 9


def test_loader_rejects_duplicate_page_and_missing_required_value(
    tmp_path: Path,
) -> None:
    rows = load_accuracy_annotations(ANNOTATIONS)
    duplicate = tmp_path / "duplicate.jsonl"
    payloads = [json.loads(line) for line in ANNOTATIONS.read_text().splitlines()]
    payloads.append(payloads[0])
    duplicate.write_text(
        "\n".join(json.dumps(row, ensure_ascii=False) for row in payloads) + "\n",
        encoding="utf-8",
    )

    with pytest.raises(ValueError, match="duplicate"):
        load_accuracy_annotations(duplicate)

    missing = tmp_path / "missing.jsonl"
    payload = json.loads(ANNOTATIONS.read_text().splitlines()[0])
    payload["transcription"] = ""
    missing.write_text(
        json.dumps(payload, ensure_ascii=False) + "\n", encoding="utf-8"
    )

    with pytest.raises(ValueError, match="transcription"):
        load_accuracy_annotations(missing)


def test_validation_rejects_source_family_leakage() -> None:
    rows = load_accuracy_annotations(ANNOTATIONS)
    leaked = replace(rows[0], source_family=rows[-1].source_family)

    with pytest.raises(ValueError, match="crosses splits"):
        validate_accuracy_corpus([leaked, *rows[1:]])


def test_local_asset_validation_checks_count_bytes_and_checksum(
    tmp_path: Path,
) -> None:
    rows = load_accuracy_annotations(ANNOTATIONS)
    selected = rows[:2]
    source_root = SERVICE_ROOT / ".data" / "corpus"
    if not all((source_root / row.asset_path).is_file() for row in selected):
        pytest.skip("ignored corpus assets are not restored")
    for row in selected:
        destination = tmp_path / row.asset_path
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source_root / row.asset_path, destination)

    evidence = validate_local_assets(selected, tmp_path)

    assert evidence["asset_count"] == 2
    assert evidence["total_bytes"] > 0
    first = tmp_path / selected[0].asset_path
    first.write_bytes(first.read_bytes() + b"tampered")
    with pytest.raises(ValueError, match="checksum"):
        validate_local_assets(selected, tmp_path)


def test_annotation_freeze_is_deterministic() -> None:
    first = load_accuracy_annotations(ANNOTATIONS)
    second = load_accuracy_annotations(ANNOTATIONS)

    assert annotation_digest(first) == annotation_digest(second)
    assert annotation_digest(first) == hashlib.sha256(
        ANNOTATIONS.read_bytes()
    ).hexdigest()
