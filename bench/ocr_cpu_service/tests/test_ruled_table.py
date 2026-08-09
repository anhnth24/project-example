from __future__ import annotations

import copy
import hashlib
import json
import sys
from pathlib import Path
from unittest.mock import patch

import pytest

sys.path.insert(0, str(Path(__file__).parents[1]))

from experiments.ruled_table import (  # noqa: E402
    CANONICAL_CONFIG_SHA256,
    CANONICAL_SOURCE_SHA256,
    CellReference,
    PageAnnotation,
    TableReference,
    load_annotation,
    load_config,
    load_manifest,
    render_frozen_pages,
    write_corpus_report,
)


SERVICE_ROOT = Path(__file__).parents[1]
CONFIG = SERVICE_ROOT / "experiments" / "ruled-table-config.json"


def valid_annotation(*, page_number: int = 450, split: str = "tuning") -> dict:
    return {
        "schema_version": 1,
        "source_sha256": "a" * 64,
        "render_sha256": "b" * 64,
        "page_number": page_number,
        "split": split,
        "negative": False,
        "review": {
            "review_status": "human_verified",
            "reviewer": "reviewer@example.invalid",
            "revision": 1,
            "reviewed_at": "2026-08-09T00:00:00Z",
        },
        "table": {
            "bbox": [10, 20, 210, 120],
            "rows": 2,
            "columns": 2,
            "cells": [
                {
                    "row": 0,
                    "column": 0,
                    "bbox": [10, 20, 110, 70],
                    "text": "Mã",
                    "blank": False,
                },
                {
                    "row": 0,
                    "column": 1,
                    "bbox": [110, 20, 210, 70],
                    "text": "Giá trị",
                    "blank": False,
                },
                {
                    "row": 1,
                    "column": 0,
                    "bbox": [10, 70, 110, 120],
                    "text": "01",
                    "blank": False,
                },
                {
                    "row": 1,
                    "column": 1,
                    "bbox": [110, 70, 210, 120],
                    "text": "",
                    "blank": True,
                },
            ],
        },
    }


def _negative_annotation(*, page_number: int) -> dict:
    payload = valid_annotation(page_number=page_number, split="holdout")
    payload["negative"] = True
    payload["table"] = None
    return payload


def _write_json(path: Path, payload: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(payload, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )


def _manifest_page(
    root: Path,
    *,
    page_number: int,
    split: str,
    negative: bool,
    template_family: str,
    annotation_payload: dict | None = None,
) -> dict:
    annotation_payload = annotation_payload or (
        _negative_annotation(page_number=page_number)
        if negative
        else valid_annotation(page_number=page_number, split=split)
    )
    annotation_payload["source_sha256"] = CANONICAL_SOURCE_SHA256
    render = root / "renders" / f"page-{page_number:04d}.png"
    annotation = root / "annotations" / f"page-{page_number:04d}.json"
    render.parent.mkdir(parents=True, exist_ok=True)
    render.write_bytes(f"render-{page_number}".encode())
    render_sha256 = hashlib.sha256(render.read_bytes()).hexdigest()
    annotation_payload["render_sha256"] = render_sha256
    _write_json(annotation, annotation_payload)
    review = annotation_payload["review"]
    return {
        "page_number": page_number,
        "split": split,
        "negative": negative,
        "template_family": template_family,
        "render_path": render.relative_to(root).as_posix(),
        "render_sha256": render_sha256,
        "annotation_path": annotation.relative_to(root).as_posix(),
        "annotation_sha256": hashlib.sha256(annotation.read_bytes()).hexdigest(),
        "review_status": review["review_status"],
        "reviewer": review["reviewer"],
        "revision": review["revision"],
    }


def write_manifest_fixture(
    tmp_path: Path,
    *,
    tuning: int = 6,
    holdout: int = 3,
    negative: int = 3,
) -> Path:
    tuning_pages = [450, 100, 200, 300, 400, 500][:tuning]
    holdout_pages = [610, 700, 790][:holdout]
    negative_pages = [60, 160, 260][:negative]
    pages = [
        _manifest_page(
            tmp_path,
            page_number=page,
            split="tuning",
            negative=False,
            template_family=f"tuning-{index}",
        )
        for index, page in enumerate(tuning_pages)
    ]
    pages += [
        _manifest_page(
            tmp_path,
            page_number=page,
            split="holdout",
            negative=False,
            template_family=f"holdout-{index}",
        )
        for index, page in enumerate(holdout_pages)
    ]
    pages += [
        _manifest_page(
            tmp_path,
            page_number=page,
            split="holdout",
            negative=True,
            template_family="negative-prose",
        )
        for page in negative_pages
    ]
    path = tmp_path / "manifest.json"
    _write_json(
        path,
        {
            "schema_version": 1,
            "source": {
                "id": "official-89-2026-tt-btc",
                "sha256": CANONICAL_SOURCE_SHA256,
                "size_bytes": 17_281_751,
            },
            "pages": pages,
        },
    )
    return path


def test_canonical_config_is_exact_and_immutable():
    assert hashlib.sha256(CONFIG.read_bytes()).hexdigest() == (
        "521efe33c8e128581708c6269e92486799201f59c648f6117048735530b0a495"
    )
    assert CANONICAL_CONFIG_SHA256 == hashlib.sha256(CONFIG.read_bytes()).hexdigest()
    config = load_config(CONFIG)
    assert tuple(item["id"] for item in config["detector_candidates"]) == (
        "strict-psm6",
        "balanced-psm6",
        "balanced-psm7",
    )
    assert config["source"]["expected_sha256"] == CANONICAL_SOURCE_SHA256
    assert config["geometry_limits"]["max_rows"] * config["geometry_limits"][
        "max_columns"
    ] <= config["geometry_limits"]["max_cells"]


@pytest.mark.parametrize(
    ("mutation", "match"),
    [
        (lambda value: value.__setitem__("unknown", True), "unknown keys"),
        (
            lambda value: value["source"].__setitem__("max_bytes", "17281751"),
            "max_bytes",
        ),
        (
            lambda value: value["source"].__setitem__("expected_sha256", "bad"),
            "SHA-256",
        ),
        (
            lambda value: value["source"].__setitem__("max_bytes", 17_281_752),
            "max_bytes",
        ),
        (
            lambda value: value["render"].__setitem__("dpi", 0),
            "dpi",
        ),
        (
            lambda value: value["render"].__setitem__("max_pixels", 50_000_001),
            "max_pixels",
        ),
        (
            lambda value: value["render"].__setitem__("max_dimension", 10_001),
            "max_dimension",
        ),
        (
            lambda value: value["geometry_limits"].__setitem__("max_rows", 51),
            "max_rows",
        ),
        (
            lambda value: value["geometry_limits"].__setitem__("max_columns", 31),
            "max_columns",
        ),
        (
            lambda value: value["geometry_limits"].__setitem__("max_cells", 1501),
            "max_cells",
        ),
        (
            lambda value: value["geometry_limits"].__setitem__("cell_match_iou", 1.1),
            "cell_match_iou",
        ),
        (
            lambda value: value["geometry_limits"].__setitem__("max_cells", 100),
            "rows.*columns.*max_cells",
        ),
        (
            lambda value: value["detector_candidates"][0].__setitem__("psm", 7),
            "candidate semantics",
        ),
        (
            lambda value: value["process_limits"].__setitem__(
                "page_timeout_seconds", 21
            ),
            "page_timeout_seconds",
        ),
        (
            lambda value: value["process_limits"].__setitem__(
                "cell_timeout_seconds", 11
            ),
            "cell_timeout_seconds",
        ),
        (
            lambda value: value["process_limits"].__setitem__(
                "max_output_bytes_per_cell", 65_537
            ),
            "max_output_bytes_per_cell",
        ),
        (
            lambda value: value["process_limits"].__setitem__(
                "max_output_bytes_per_page", 1_048_577
            ),
            "max_output_bytes_per_page",
        ),
        (
            lambda value: value["process_limits"].__setitem__(
                "max_rss_bytes", 805_306_369
            ),
            "max_rss_bytes",
        ),
        (
            lambda value: value["process_limits"].__setitem__(
                "sample_interval_ms", 11
            ),
            "sample_interval_ms",
        ),
        (
            lambda value: value["detector_candidates"].reverse(),
            "candidate IDs",
        ),
        (
            lambda value: value["gate"].__setitem__(
                "maximum_negative_false_positives", -1
            ),
            "maximum_negative_false_positives",
        ),
    ],
)
def test_config_rejects_invalid_or_noncanonical_values(tmp_path, mutation, match):
    payload = json.loads(CONFIG.read_text(encoding="utf-8"))
    mutation(payload)
    path = tmp_path / "config.json"
    _write_json(path, payload)
    with pytest.raises(ValueError, match=match):
        load_config(path)


def test_annotation_returns_frozen_typed_contract(tmp_path):
    path = tmp_path / "annotation.json"
    _write_json(path, valid_annotation())
    result = load_annotation(path, expected_render_sha256="b" * 64)
    assert result == PageAnnotation(
        page_number=450,
        split="tuning",
        negative=False,
        source_sha256="a" * 64,
        render_sha256="b" * 64,
        review_status="human_verified",
        reviewer="reviewer@example.invalid",
        revision=1,
        table=TableReference(
            bbox=(10, 20, 210, 120),
            rows=2,
            columns=2,
            cells=(
                CellReference(0, 0, (10, 20, 110, 70), "Mã", False),
                CellReference(0, 1, (110, 20, 210, 70), "Giá trị", False),
                CellReference(1, 0, (10, 70, 110, 120), "01", False),
                CellReference(1, 1, (110, 70, 210, 120), "", True),
            ),
        ),
    )
    with pytest.raises(AttributeError):
        result.page_number = 1


def test_annotation_requires_complete_non_overlapping_matrix(tmp_path):
    annotation = valid_annotation()
    annotation["table"]["cells"].pop()
    path = tmp_path / "annotation.json"
    _write_json(path, annotation)
    with pytest.raises(ValueError, match="complete rectangular matrix"):
        load_annotation(path, expected_render_sha256="b" * 64)


def test_holdout_rejects_non_human_review(tmp_path):
    annotation = valid_annotation(split="holdout")
    annotation["review"] = {
        "review_status": "draft",
        "reviewer": "unassigned",
        "revision": 0,
        "reviewed_at": None,
    }
    path = tmp_path / "annotation.json"
    _write_json(path, annotation)
    with pytest.raises(ValueError, match="human_verified"):
        load_annotation(path, expected_render_sha256="b" * 64)


@pytest.mark.parametrize(
    ("mutation", "match"),
    [
        (lambda value: value.__setitem__("unknown", True), "unknown keys"),
        (lambda value: value.pop("source_sha256"), "missing keys"),
        (lambda value: value.__setitem__("page_number", True), "page_number"),
        (lambda value: value.__setitem__("source_sha256", "no"), "SHA-256"),
        (
            lambda value: value["table"]["cells"][0].__setitem__("row", "0"),
            "row",
        ),
        (
            lambda value: value["table"]["cells"][3].__setitem__("row", 0),
            "complete rectangular matrix",
        ),
        (
            lambda value: value["table"]["cells"][3].__setitem__(
                "bbox", [100, 60, 200, 110]
            ),
            "overlapping cells",
        ),
        (
            lambda value: value["table"]["cells"][3].__setitem__(
                "bbox", [110, 70, 220, 120]
            ),
            "table bounds",
        ),
        (
            lambda value: value["table"]["cells"][3].__setitem__("text", "not blank"),
            "blank cell",
        ),
        (
            lambda value: value["review"].__setitem__("reviewer", ""),
            "reviewer",
        ),
    ],
)
def test_annotation_rejects_invalid_closed_schema(tmp_path, mutation, match):
    annotation = valid_annotation()
    mutation(annotation)
    path = tmp_path / "annotation.json"
    _write_json(path, annotation)
    with pytest.raises(ValueError, match=match):
        load_annotation(path, expected_render_sha256="b" * 64)


def test_annotation_rejects_render_hash_mismatch(tmp_path):
    path = tmp_path / "annotation.json"
    _write_json(path, valid_annotation())
    with pytest.raises(ValueError, match="render SHA-256 mismatch"):
        load_annotation(path, expected_render_sha256="c" * 64)


def test_negative_annotation_has_no_table(tmp_path):
    annotation = _negative_annotation(page_number=60)
    path = tmp_path / "annotation.json"
    _write_json(path, annotation)
    result = load_annotation(path, expected_render_sha256="b" * 64)
    assert result.negative is True
    assert result.table is None


def test_tuning_mode_cannot_open_holdout_annotations(tmp_path):
    manifest = write_manifest_fixture(tmp_path, tuning=6, holdout=3, negative=3)
    loaded = load_manifest(manifest, mode="tuning")
    with pytest.raises(PermissionError, match="holdout access denied"):
        loaded.open_page(700)
    assert loaded.access_counts == {"tuning": 0, "holdout": 0, "negative": 0}


def test_tuning_mode_denies_holdout_before_filesystem_open(tmp_path):
    manifest = write_manifest_fixture(tmp_path)
    loaded = load_manifest(manifest, mode="tuning")
    holdout = next(item for item in loaded.holdout if item.page_number == 700)
    (tmp_path / holdout.render_path).unlink()
    (tmp_path / holdout.annotation_path).unlink()
    with pytest.raises(PermissionError, match="holdout access denied"):
        loaded.open_page(700)


def test_open_page_hashes_annotation_and_render_and_counts_access(tmp_path):
    manifest = write_manifest_fixture(tmp_path)
    loaded = load_manifest(manifest, mode="tuning")
    opened = loaded.open_page(450)
    assert opened.page.page_number == 450
    assert opened.annotation.page_number == 450
    assert loaded.access_counts == {"tuning": 1, "holdout": 0, "negative": 0}

    annotation_path = tmp_path / opened.page.annotation_path
    annotation_path.write_text("{}", encoding="utf-8")
    with pytest.raises(ValueError, match="annotation SHA-256 mismatch"):
        loaded.open_page(450)

    page = next(item for item in loaded.tuning if item.page_number == 100)
    (tmp_path / page.render_path).write_bytes(b"tampered")
    with pytest.raises(ValueError, match="render SHA-256 mismatch"):
        loaded.open_page(100)


@pytest.mark.parametrize(
    ("mutate", "match"),
    [
        (lambda value: value.__setitem__("unexpected", 1), "unknown keys"),
        (
            lambda value: value["source"].__setitem__("size_bytes", "17281751"),
            "size_bytes",
        ),
        (
            lambda value: value["source"].__setitem__("sha256", "bad"),
            "SHA-256",
        ),
        (
            lambda value: value["pages"][1].__setitem__(
                "page_number", value["pages"][0]["page_number"]
            ),
            "duplicate page",
        ),
        (
            lambda value: value["pages"][0].__setitem__("page_number", 451),
            "page 450.*tuning",
        ),
        (
            lambda value: value["pages"][6].__setitem__(
                "template_family", value["pages"][0]["template_family"]
            ),
            "template leakage",
        ),
        (
            lambda value: value["pages"][6].__setitem__("page_number", 501),
            "adjacent-template leakage",
        ),
        (
            lambda value: value["pages"].pop(),
            "6 tuning.*3 holdout.*3 negative",
        ),
    ],
)
def test_manifest_rejects_invalid_closed_schema(tmp_path, mutate, match):
    path = write_manifest_fixture(tmp_path)
    payload = json.loads(path.read_text(encoding="utf-8"))
    mutate(payload)
    _write_json(path, payload)
    with pytest.raises(ValueError, match=match):
        load_manifest(path, mode="tuning")


def test_manifest_exposes_exact_frozen_splits(tmp_path):
    manifest = load_manifest(write_manifest_fixture(tmp_path), mode="holdout")
    assert manifest.source_sha256 == CANONICAL_SOURCE_SHA256
    assert len(manifest.tuning) == 6
    assert len(manifest.holdout) == 3
    assert len(manifest.negative) == 3
    assert isinstance(manifest.tuning, tuple)


def test_render_rejects_source_size_before_read_or_hash(tmp_path):
    source = tmp_path / "oversized.pdf"
    source.write_bytes(b"x" * 16)
    config = json.loads(CONFIG.read_text(encoding="utf-8"))
    config["source"]["max_bytes"] = 15
    config_path = tmp_path / "config.json"
    _write_json(config_path, config)
    original_read_bytes = Path.read_bytes

    def guarded_read_bytes(path: Path) -> bytes:
        if path == source:
            raise AssertionError("must not read oversized source")
        return original_read_bytes(path)

    with patch.object(Path, "read_bytes", guarded_read_bytes):
        with pytest.raises(ValueError, match="source size limit"):
            render_frozen_pages(
                source,
                pages=[],
                output_root=tmp_path / "output",
                manifest_path=tmp_path / "manifest.json",
                config_path=config_path,
            )


def test_report_is_exact_and_private_annotation_data_never_leaks(tmp_path):
    path = write_manifest_fixture(tmp_path)
    payload = json.loads(path.read_text(encoding="utf-8"))
    payload["pages"][0]["annotation_path"] = "private/unique-customer-path.json"
    payload["pages"][0]["annotation_sha256"] = "d" * 64
    payload["pages"][0]["reviewer"] = "UNIQUE PRIVATE ANNOTATION PHRASE"
    # The report uses only frozen aggregate metadata and never opens annotation content.
    _write_json(path, payload)
    manifest = load_manifest(path, mode="tuning")
    output = tmp_path / "report.md"
    write_corpus_report(manifest, output)
    report = output.read_text(encoding="utf-8")
    expected_manifest_hash = hashlib.sha256(path.read_bytes()).hexdigest()
    assert report == (
        "# Ruled-table OCR corpus\n"
        "\n"
        "- Source id: `official-89-2026-tt-btc`\n"
        f"- Source SHA-256: `{CANONICAL_SOURCE_SHA256}`\n"
        f"- Manifest SHA-256: `{expected_manifest_hash}`\n"
        "- Tuning table pages: 6\n"
        "- Holdout table pages: 3\n"
        "- Negative pages: 3\n"
        "- Human-verified annotations: 12/12\n"
        "- Distinct template families: 10\n"
        "- Ground-truth text tracked: no\n"
    )
    assert "UNIQUE PRIVATE ANNOTATION PHRASE" not in report
    assert "private/unique-customer-path.json" not in report
    assert "d" * 64 not in report
