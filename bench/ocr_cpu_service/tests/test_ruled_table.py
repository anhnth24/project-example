from __future__ import annotations

import copy
import hashlib
import io
import json
import shutil
import sys
from pathlib import Path
from unittest.mock import patch

import pytest
from PIL import Image

sys.path.insert(0, str(Path(__file__).parents[1]))

from benchmark.metrics import error_counts  # noqa: E402
from experiments.ruled_table import (  # noqa: E402
    CANONICAL_CONFIG_SHA256,
    CANONICAL_SOURCE_SHA256,
    ArtifactBindings,
    CellContentCounts,
    CellMatchCounts,
    CellReference,
    ManifestPage,
    OpenedPage,
    PageAnnotation,
    PositionedBox,
    TableReference,
    _aggregate_candidate,
    _empty_cell_evidence,
    _parser,
    _recompute_aggregates,
    _run_page,
    derive_holdout_decision,
    derive_tuning_winner,
    freeze_tuning_winner,
    load_annotation,
    load_config,
    load_manifest,
    main,
    match_cells,
    measure_cell_content,
    render_frozen_pages,
    render_report,
    run_split,
    template_fingerprint,
    validate_artifact,
    validate_frozen_winner,
    write_corpus_report,
)
from experiments.table_cells import (  # noqa: E402
    TableCleanupError,
    TableRecognitionError,
)
from experiments.table_lines import (  # noqa: E402
    Box,
    DetectionResult,
    Grid,
    GridCell,
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
    if annotation_payload is None:
        annotation_payload = (
            _negative_annotation(page_number=page_number)
            if negative
            else valid_annotation(page_number=page_number, split=split)
        )
        if not negative:
            midpoint = 20 + (page_number // 10) % 180
            annotation_payload["table"]["cells"][0]["bbox"][2] = midpoint
            annotation_payload["table"]["cells"][1]["bbox"][0] = midpoint
            annotation_payload["table"]["cells"][2]["bbox"][2] = midpoint
            annotation_payload["table"]["cells"][3]["bbox"][0] = midpoint
    annotation_payload["source_sha256"] = CANONICAL_SOURCE_SHA256
    render = root / "renders" / f"page-{page_number:04d}.png"
    annotation = root / "annotations" / f"page-{page_number:04d}.json"
    render.parent.mkdir(parents=True, exist_ok=True)
    render.write_bytes(f"render-{page_number}".encode())
    render_sha256 = hashlib.sha256(render.read_bytes()).hexdigest()
    annotation_payload["render_sha256"] = render_sha256
    _write_json(annotation, annotation_payload)
    review = annotation_payload["review"]
    parsed_annotation = load_annotation(
        annotation,
        expected_render_sha256=render_sha256,
    )
    return {
        "page_number": page_number,
        "split": split,
        "negative": negative,
        "template_family": template_family,
        "template_fingerprint": template_fingerprint(parsed_annotation),
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
        "53882ed34ec756fd2fc9e7bb3ad66ac021c86b26c70a04299f8dd1b1eec0a3f8"
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


def _set_config_path(payload: dict, path: tuple[object, ...], value: object) -> None:
    current: object = payload
    for component in path[:-1]:
        current = current[component]
    current[path[-1]] = value


@pytest.mark.parametrize(
    ("path", "value"),
    [
        (("source", "id"), "other-source"),
        (("source", "expected_sha256"), "a" * 64),
        (("source", "max_bytes"), 17_281_750),
        (("render", "dpi"), 299),
        (("render", "max_pixels"), 49_999_999),
        (("render", "max_dimension"), 9_999),
        (("geometry_limits", "max_rows"), 49),
        (("geometry_limits", "max_columns"), 29),
        (("geometry_limits", "max_cells"), 1_499),
        (("geometry_limits", "max_table_regions"), 2),
        (("geometry_limits", "cell_match_iou"), 0.79),
        (("process_limits", "cpu_threads"), 2),
        (("process_limits", "page_timeout_seconds"), 19),
        (("process_limits", "cell_timeout_seconds"), 9),
        (("process_limits", "max_output_bytes_per_cell"), 65_535),
        (("process_limits", "max_output_bytes_per_page"), 1_048_575),
        (("process_limits", "max_rss_bytes"), 805_306_367),
        (("process_limits", "sample_interval_ms"), 9),
        (("detector_candidates", 0, "id"), "strict-renamed"),
        (("detector_candidates", 1, "id"), "balanced-renamed"),
        (("detector_candidates", 2, "id"), "balanced-psm8"),
        (("detector_candidates", 1, "dark_max"), 159),
        (("detector_candidates", 1, "min_horizontal_fraction"), 0.19),
        (("detector_candidates", 1, "min_vertical_fraction"), 0.07),
        (("detector_candidates", 1, "max_gap_pixels"), 11),
        (("detector_candidates", 1, "cluster_tolerance_pixels"), 4),
        (("detector_candidates", 1, "intersection_tolerance_pixels"), 4),
        (
            ("detector_candidates", 1, "deskew_angles_degrees"),
            [-1.0, 0.0, 1.0],
        ),
        (("detector_candidates", 1, "cell_inset_pixels"), 3),
        (("detector_candidates", 1, "psm"), 7),
        (("gate", "exact_grid_required"), False),
        (("gate", "minimum_cell_f1"), 0.94),
        (("gate", "maximum_cell_cer"), 0.06),
        (("gate", "minimum_empty_cell_accuracy"), 0.97),
        (("gate", "maximum_negative_false_positives"), 1),
    ],
)
def test_config_rejects_every_nested_semantic_drift(tmp_path, path, value):
    payload = json.loads(CONFIG.read_text(encoding="utf-8"))
    _set_config_path(payload, path, value)
    altered = tmp_path / "altered.json"
    _write_json(altered, payload)
    with pytest.raises(ValueError, match="canonical|semantics|candidate"):
        load_config(altered)


def test_config_accepts_only_byte_identical_custom_copy(tmp_path):
    copied = tmp_path / "copied.json"
    copied.write_bytes(CONFIG.read_bytes())
    assert load_config(copied) == load_config(CONFIG)

    copied.write_bytes(CONFIG.read_bytes() + b" ")
    with pytest.raises(ValueError, match="canonical config SHA-256"):
        load_config(copied)


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


def test_template_fingerprint_uses_normalized_geometry(tmp_path):
    original_path = tmp_path / "original.json"
    _write_json(original_path, valid_annotation())
    original = load_annotation(
        original_path,
        expected_render_sha256="b" * 64,
    )

    scaled = valid_annotation()
    scaled["table"]["bbox"] = [110, 220, 510, 420]
    for cell in scaled["table"]["cells"]:
        x1, y1, x2, y2 = cell["bbox"]
        cell["bbox"] = [
            110 + (x1 - 10) * 2,
            220 + (y1 - 20) * 2,
            110 + (x2 - 10) * 2,
            220 + (y2 - 20) * 2,
        ]
    scaled_path = tmp_path / "scaled.json"
    _write_json(scaled_path, scaled)
    scaled_annotation = load_annotation(
        scaled_path,
        expected_render_sha256="b" * 64,
    )
    assert template_fingerprint(original) == template_fingerprint(scaled_annotation)

    changed = copy.deepcopy(scaled)
    changed["table"]["cells"][0]["bbox"][2] = 290
    changed["table"]["cells"][1]["bbox"][0] = 290
    changed["table"]["cells"][2]["bbox"][2] = 290
    changed["table"]["cells"][3]["bbox"][0] = 290
    changed_path = tmp_path / "changed.json"
    _write_json(changed_path, changed)
    changed_annotation = load_annotation(
        changed_path,
        expected_render_sha256="b" * 64,
    )
    assert template_fingerprint(original) != template_fingerprint(changed_annotation)


@pytest.mark.parametrize(
    ("rows", "columns", "match"),
    [
        (51, 1, "max_rows"),
        (1, 31, "max_columns"),
    ],
)
def test_annotation_rejects_geometry_dimensions_before_matrix_allocation(
    tmp_path, rows, columns, match
):
    annotation = valid_annotation()
    annotation["table"]["rows"] = rows
    annotation["table"]["columns"] = columns
    path = tmp_path / "annotation.json"
    _write_json(path, annotation)
    with pytest.raises(ValueError, match=match):
        load_annotation(path, expected_render_sha256="b" * 64)


def test_annotation_rejects_cell_overflow_before_cell_validation(tmp_path):
    annotation = valid_annotation()
    annotation["table"]["rows"] = 50
    annotation["table"]["columns"] = 30
    annotation["table"]["cells"] = [
        {"not": "a valid cell"} for _ in range(1_501)
    ]
    path = tmp_path / "annotation.json"
    _write_json(path, annotation)
    with pytest.raises(ValueError, match="maximum 1500 cells"):
        load_annotation(path, expected_render_sha256="b" * 64)


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


def test_manifest_rejects_duplicate_structural_fingerprint_despite_distinct_ids(
    tmp_path,
):
    path = write_manifest_fixture(tmp_path)
    payload = json.loads(path.read_text(encoding="utf-8"))
    payload["pages"][6]["template_fingerprint"] = payload["pages"][0][
        "template_fingerprint"
    ]
    assert payload["pages"][6]["template_family"] != payload["pages"][0][
        "template_family"
    ]
    _write_json(path, payload)
    with pytest.raises(ValueError, match="duplicate structural template fingerprint"):
        load_manifest(path, mode="tuning")


def test_open_page_verifies_fingerprint_against_annotation(tmp_path):
    path = write_manifest_fixture(tmp_path)
    payload = json.loads(path.read_text(encoding="utf-8"))
    page = next(item for item in payload["pages"] if item["page_number"] == 450)
    page["template_fingerprint"] = "f" * 64
    _write_json(path, payload)
    loaded = load_manifest(path, mode="tuning")
    with pytest.raises(ValueError, match="template fingerprint mismatch"):
        loaded.open_page(450)


def test_manifest_exposes_exact_frozen_splits(tmp_path):
    manifest = load_manifest(write_manifest_fixture(tmp_path), mode="holdout")
    assert manifest.source_sha256 == CANONICAL_SOURCE_SHA256
    assert len(manifest.tuning) == 6
    assert len(manifest.holdout) == 3
    assert len(manifest.negative) == 3
    assert isinstance(manifest.tuning, tuple)


def test_render_rejects_source_size_before_read_or_hash(tmp_path):
    source = tmp_path / "oversized.pdf"
    source.write_bytes(b"x")
    original_read_bytes = Path.read_bytes
    original_stat = Path.stat

    def guarded_read_bytes(path: Path) -> bytes:
        if path == source:
            raise AssertionError("must not read oversized source")
        return original_read_bytes(path)

    def oversized_stat(path: Path):
        result = original_stat(path)
        if path == source:
            return type("OversizedStat", (), {"st_size": 17_281_752})()
        return result

    with (
        patch.object(Path, "read_bytes", guarded_read_bytes),
        patch.object(Path, "stat", oversized_stat),
    ):
        with pytest.raises(ValueError, match="source size limit"):
            render_frozen_pages(
                source,
                pages=[],
                output_root=tmp_path / "output",
                manifest_path=tmp_path / "manifest.json",
                config_path=CONFIG,
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


def _canonical_json_sha256(value: object) -> str:
    encoded = json.dumps(
        value, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _artifact_bindings(*, render_sha256: str = "c" * 64) -> dict:
    return {
        "config_sha256": CANONICAL_CONFIG_SHA256,
        "manifest_sha256": "b" * 64,
        "source_sha256": CANONICAL_SOURCE_SHA256,
        "render_sha256": render_sha256,
        "tesseract_sha256": "d" * 64,
        "tessdata_sha256": "e" * 64,
        "host_sha256": _canonical_json_sha256(
            {
                "platform": "OCR_SECRET_PLATFORM",
                "architecture": "reference-secret-architecture",
                "logical_cpus": 4,
                "memory_bytes": 1_000_000_000,
            }
        ),
        "toolchain_sha256": _canonical_json_sha256(
            {
                "python": "PATH_SECRET_PYTHON",
                "pillow": "ENV_SECRET_PILLOW",
                "tesseract": "5.3.4",
            }
        ),
    }


def _candidate_descriptors() -> list[dict]:
    config = json.loads(CONFIG.read_text(encoding="utf-8"))
    return [
        {
            "id": candidate["id"],
            "configuration_sha256": _canonical_json_sha256(candidate),
        }
        for candidate in config["detector_candidates"]
    ]


def _record(
    candidate_id: str,
    page_number: int,
    *,
    split: str,
    negative: bool = False,
    status: str | None = None,
    elapsed_seconds: float = 1.0,
    peak_rss_bytes: int = 100,
) -> dict:
    if status is None:
        status = "not_detected" if negative else "detected"
    table = not negative
    return {
        "id": f"{split}:{candidate_id}:{page_number}",
        "candidate_id": candidate_id,
        "page_number": page_number,
        "split": split,
        "negative": negative,
        "status": status,
        "rows": 1 if table and status == "detected" else 0,
        "columns": 1 if table and status == "detected" else 0,
        "reference_rows": 1 if table else 0,
        "reference_columns": 1 if table else 0,
        "predicted_boxes": (
            [{"row": 0, "column": 0, "bbox": [0, 0, 100, 100]}]
            if table and status == "detected"
            else []
        ),
        "reference_boxes": (
            [{"row": 0, "column": 0, "bbox": [0, 0, 100, 100]}]
            if table
            else []
        ),
        "cells": (
            [
                {
                    "row": 0,
                    "column": 0,
                    "character_edits": 0,
                    "reference_characters": 10,
                    "word_edits": 0,
                    "reference_words": 2,
                    "reference_blank": False,
                    "predicted_blank": False,
                    "prediction_present": True,
                }
            ]
            if table and status == "detected"
            else []
        ),
        "elapsed_seconds": elapsed_seconds,
        "peak_rss_bytes": peak_rss_bytes,
        "resource": {
            "timed_out": status == "timeout",
            "output_limited": status == "output_limit",
            "candidate_failed": status == "candidate_error",
            "resource_violation": status == "resource_limit",
            "cleanup_failed": status == "cleanup_error",
            "primary_error_kind": (
                status
                if status
                in {
                    "invalid_grid",
                    "timeout",
                    "output_limit",
                    "candidate_error",
                    "resource_limit",
                    "cleanup_error",
                }
                else None
            ),
        },
        "bindings": _artifact_bindings(),
    }


def _aggregate(candidate_id: str, records: list[dict]) -> dict:
    tables = [record for record in records if not record["negative"]]
    negatives = [record for record in records if record["negative"]]
    cells = [cell for record in records for cell in record["cells"]]
    tp = sum(len(record["predicted_boxes"]) for record in tables)
    reference_boxes = sum(len(record["reference_boxes"]) for record in tables)
    fp = sum(
        max(0, len(record["predicted_boxes"]) - len(record["reference_boxes"]))
        for record in tables
    )
    fn = reference_boxes - tp + fp
    precision = tp / (tp + fp) if tp + fp else 1.0
    recall = tp / (tp + fn) if tp + fn else 1.0
    f1 = (
        2 * precision * recall / (precision + recall)
        if precision + recall
        else 0.0
    )
    character_edits = sum(cell["character_edits"] for cell in cells)
    reference_characters = sum(cell["reference_characters"] for cell in cells)
    word_edits = sum(cell["word_edits"] for cell in cells)
    reference_words = sum(cell["reference_words"] for cell in cells)
    blank_correct = sum(
        cell["reference_blank"] == cell["predicted_blank"] for cell in cells
    )
    elapsed = sorted(record["elapsed_seconds"] for record in records)
    median = (
        elapsed[len(elapsed) // 2]
        if len(elapsed) % 2
        else (elapsed[len(elapsed) // 2 - 1] + elapsed[len(elapsed) // 2]) / 2
    )
    successful = {"detected", "not_detected"}
    exact_grid_pages = sum(
        record["status"] == "detected"
        and record["rows"] == record["reference_rows"]
        and record["columns"] == record["reference_columns"]
        for record in tables
    )
    return {
        "candidate_id": candidate_id,
        "record_count": len(records),
        "table_pages": len(tables),
        "negative_pages": len(negatives),
        "failures": sum(record["status"] not in successful for record in records),
        "timeouts": sum(record["resource"]["timed_out"] for record in records),
        "output_limit_failures": sum(
            record["resource"]["output_limited"] for record in records
        ),
        "candidate_failures": sum(
            record["resource"]["candidate_failed"] for record in records
        ),
        "resource_violations": sum(
            record["resource"]["resource_violation"] for record in records
        ),
        "cleanup_failures": sum(
            record["resource"]["cleanup_failed"] for record in records
        ),
        "false_positives": sum(
            record["status"] == "detected" for record in negatives
        ),
        "wrong_grids": len(tables) - exact_grid_pages,
        "exact_grid_pages": exact_grid_pages,
        "cell_tp": tp,
        "cell_fp": fp,
        "cell_fn": fn,
        "character_edits": character_edits,
        "reference_characters": reference_characters,
        "word_edits": word_edits,
        "reference_words": reference_words,
        "blank_correct": blank_correct,
        "blank_total": len(cells),
        "cell_precision": precision,
        "cell_recall": recall,
        "cell_f1": f1,
        "cell_cer": (
            character_edits / reference_characters
            if reference_characters
            else float(character_edits > 0)
        ),
        "cell_wer": (
            word_edits / reference_words
            if reference_words
            else float(word_edits > 0)
        ),
        "empty_cell_accuracy": blank_correct / len(cells) if cells else 1.0,
        "median_page_latency_seconds": median,
        "maximum_page_latency_seconds": max(elapsed),
        "peak_rss_bytes": max(record["peak_rss_bytes"] for record in records),
    }


def artifact_fixture(split: str = "holdout") -> dict:
    candidate_ids = [item["id"] for item in _candidate_descriptors()]
    if split == "tuning":
        records = [
            _record(candidate_id, page, split="tuning")
            for candidate_id in candidate_ids
            for page in (255, 460, 537, 450, 541, 772)
        ]
        aggregates = [
            _aggregate(
                candidate_id,
                [row for row in records if row["candidate_id"] == candidate_id],
            )
            for candidate_id in candidate_ids
        ]
        winner_id = "balanced-psm6"
        decision = None
        access = {
            "tuning_pages_opened": 6,
            "holdout_pages_opened": 0,
            "negative_pages_opened": 0,
        }
    else:
        winner_id = "balanced-psm6"
        records = [
            *[
                _record(winner_id, page, split="holdout")
                for page in (394, 503, 809)
            ],
            *[
                _record(winner_id, page, split="holdout", negative=True)
                for page in (20, 60, 100)
            ],
        ]
        aggregates = [_aggregate(winner_id, records)]
        decision = "PASS"
        access = {
            "tuning_pages_opened": 0,
            "holdout_pages_opened": 3,
            "negative_pages_opened": 3,
        }
    return {
        "schema_version": 1,
        "split": split,
        "source": {
            "id": "official-89-2026-tt-btc",
            "sha256": CANONICAL_SOURCE_SHA256,
            "size_bytes": 17_281_751,
        },
        "config_sha256": CANONICAL_CONFIG_SHA256,
        "manifest_sha256": "b" * 64,
        "host": {
            "platform": "OCR_SECRET_PLATFORM",
            "architecture": "reference-secret-architecture",
            "logical_cpus": 4,
            "memory_bytes": 1_000_000_000,
        },
        "toolchain": {
            "python": "PATH_SECRET_PYTHON",
            "pillow": "ENV_SECRET_PILLOW",
            "tesseract": "5.3.4",
        },
        "access": access,
        "candidates": _candidate_descriptors(),
        "records": records,
        "aggregates": aggregates,
        "winner_id": winner_id,
        "decision": decision,
    }


def no_winner_tuning_artifact() -> dict:
    payload = artifact_fixture("tuning")
    for record in payload["records"]:
        record["status"] = "invalid_grid"
        record["rows"] = 0
        record["columns"] = 0
        record["predicted_boxes"] = []
        record["resource"]["primary_error_kind"] = "invalid_grid"
        for cell in record["cells"]:
            cell["character_edits"] = cell["reference_characters"]
            cell["word_edits"] = cell["reference_words"]
            cell["predicted_blank"] = False
            cell["prediction_present"] = False
    payload["aggregates"] = _recompute_aggregates(payload)
    payload["winner_id"] = None
    return payload


def _canonical_bindings_fixture() -> ArtifactBindings:
    values = _artifact_bindings()
    return ArtifactBindings(
        config_sha256=values["config_sha256"],
        manifest_sha256=values["manifest_sha256"],
        source_sha256=values["source_sha256"],
        render_sha256_by_page={
            page: values["render_sha256"]
            for page in (20, 60, 100, 255, 394, 450, 460, 503, 537, 541, 772, 809)
        },
        tesseract_sha256=values["tesseract_sha256"],
        tessdata_sha256=values["tessdata_sha256"],
        host_sha256=values["host_sha256"],
        toolchain_sha256=values["toolchain_sha256"],
    )


def test_iou_matching_is_one_to_one_at_exact_threshold():
    references = [Box(0, 0, 100, 100), Box(100, 0, 200, 100)]
    predictions = [Box(0, 0, 80, 100), Box(0, 0, 100, 100)]
    counts = match_cells(references, predictions, threshold=0.80)
    assert counts == CellMatchCounts(tp=1, fp=1, fn=1)


def test_iou_empty_sets_score_as_perfect_after_additive_counts():
    counts = match_cells([], [], threshold=0.80)
    assert counts == CellMatchCounts(tp=0, fp=0, fn=0)
    assert counts.precision == counts.recall == counts.f1 == 1.0


def test_content_counts_are_additive_for_missing_and_extra_cells():
    counts = measure_cell_content(
        {
            (0, 0): ("Việt Nam", False),
            (0, 1): ("", True),
        },
        {
            (0, 0): "Việt Nam",
            (1, 0): "thừa",
        },
    )
    assert counts == CellContentCounts(
        character_edits=4,
        reference_characters=8,
        word_edits=1,
        reference_words=2,
        blank_correct=1,
        blank_total=2,
        cells={
            (0, 0): (0, 8, 0, 2, False, False),
            (0, 1): (0, 0, 0, 0, True, False),
            (1, 0): (4, 0, 1, 0, False, False),
        },
    )


def test_content_alignment_requires_confirmed_exact_grid():
    with pytest.raises(ValueError, match="exact rows and columns"):
        measure_cell_content(
            {(0, 0): ("a", False)},
            {(0, 0): "a"},
            reference_shape=(1, 1),
            predicted_shape=(1, 2),
        )


def test_tuning_winner_uses_exact_ranking_and_lexical_tie_break():
    payload = artifact_fixture("tuning")
    assert derive_tuning_winner(payload) == "balanced-psm6"
    payload["aggregates"][0]["exact_grid_pages"] = 5
    assert derive_tuning_winner(payload) == "balanced-psm6"
    payload["aggregates"][1]["failures"] = 1
    assert derive_tuning_winner(payload) == "balanced-psm7"


def test_holdout_gate_requires_every_condition():
    payload = artifact_fixture()
    assert derive_holdout_decision(payload) == "PASS"
    payload["aggregates"][0]["cell_cer"] = 0.050001
    assert derive_holdout_decision(payload) == "STOP"


@pytest.mark.parametrize(
    ("field", "passing", "stopping"),
    [
        ("cell_f1", 0.95, 0.949999),
        ("cell_cer", 0.05, 0.050001),
        ("empty_cell_accuracy", 0.98, 0.979999),
        ("peak_rss_bytes", 805_306_367, 805_306_368),
        ("maximum_page_latency_seconds", 20.0, 20.000001),
    ],
)
def test_holdout_gate_exact_boundaries(field, passing, stopping):
    payload = artifact_fixture()
    payload["aggregates"][0][field] = passing
    assert derive_holdout_decision(payload) == "PASS"
    payload["aggregates"][0][field] = stopping
    assert derive_holdout_decision(payload) == "STOP"


def test_negative_false_positive_forces_stop():
    payload = artifact_fixture()
    payload["records"][-1]["status"] = "detected"
    assert derive_holdout_decision(payload) == "STOP"


def test_validate_artifact_accepts_closed_external_bound_artifact():
    payload = artifact_fixture()
    with patch(
        "experiments.ruled_table._canonical_artifact_bindings",
        return_value=_canonical_bindings_fixture(),
    ):
        validate_artifact(payload, split="holdout")


@pytest.mark.parametrize(
    "mutation",
    [
        lambda value: value["records"].pop(),
        lambda value: value["records"].append(copy.deepcopy(value["records"][0])),
        lambda value: value["records"][0]["cells"][0].__setitem__("unknown", 1),
        lambda value: value["records"][0]["cells"][0].__setitem__(
            "recognized_text", "OCR_TEXT_SECRET"
        ),
        lambda value: value["records"][0].__setitem__(
            "reference", "REFERENCE_SECRET"
        ),
        lambda value: value["records"][0].__setitem__("markdown", "MARKDOWN_SECRET"),
        lambda value: value["records"][0].__setitem__(
            "environment", {"TOKEN": "ENV_SECRET"}
        ),
    ],
)
def test_validate_artifact_rejects_cardinality_and_recursive_schema_drift(mutation):
    payload = artifact_fixture()
    mutation(payload)
    with (
        patch(
            "experiments.ruled_table._canonical_artifact_bindings",
            return_value=_canonical_bindings_fixture(),
        ),
        pytest.raises(ValueError),
    ):
        validate_artifact(payload, split="holdout")


def test_validate_artifact_rejects_stale_aggregate_and_decision():
    payload = artifact_fixture()
    payload["aggregates"][0]["cell_tp"] += 1
    with (
        patch(
            "experiments.ruled_table._canonical_artifact_bindings",
            return_value=_canonical_bindings_fixture(),
        ),
        pytest.raises(ValueError, match="aggregate"),
    ):
        validate_artifact(payload, split="holdout")

    payload = artifact_fixture()
    payload["decision"] = "STOP"
    with (
        patch(
            "experiments.ruled_table._canonical_artifact_bindings",
            return_value=_canonical_bindings_fixture(),
        ),
        pytest.raises(ValueError, match="decision"),
    ):
        validate_artifact(payload, split="holdout")


@pytest.mark.parametrize(
    ("path", "value"),
    [
        (("config_sha256",), "f" * 64),
        (("manifest_sha256",), "f" * 64),
        (("source", "sha256"), "f" * 64),
        (("host", "logical_cpus"), 5),
        (("toolchain", "python"), "different"),
        (("records", 0, "bindings", "render_sha256"), "f" * 64),
        (("records", 0, "bindings", "tesseract_sha256"), "f" * 64),
        (("records", 0, "bindings", "tessdata_sha256"), "f" * 64),
    ],
)
def test_validate_artifact_rejects_wrong_external_hashes(path, value):
    payload = artifact_fixture()
    _set_config_path(payload, path, value)
    with (
        patch(
            "experiments.ruled_table._canonical_artifact_bindings",
            return_value=_canonical_bindings_fixture(),
        ),
        pytest.raises(ValueError, match="hash|SHA|bind|canonical"),
    ):
        validate_artifact(payload, split="holdout")


def test_validate_tuning_requires_access_six_zero_zero_and_all_candidate_records():
    payload = artifact_fixture("tuning")
    payload["access"]["holdout_pages_opened"] = 1
    with (
        patch(
            "experiments.ruled_table._canonical_artifact_bindings",
            return_value=_canonical_bindings_fixture(),
        ),
        pytest.raises(ValueError, match="access"),
    ):
        validate_artifact(payload, split="tuning")


def test_report_is_deterministic_aggregate_only_and_private():
    tuning = artifact_fixture("tuning")
    holdout = artifact_fixture("holdout")
    with patch(
        "experiments.ruled_table._canonical_artifact_bindings",
        return_value=_canonical_bindings_fixture(),
    ):
        report_one = render_report(tuning, holdout)
        report_two = render_report(
            copy.deepcopy(tuning), copy.deepcopy(holdout)
        )
    assert report_one.encode("utf-8") == report_two.encode("utf-8")
    for secret in (
        "OCR_SECRET_PLATFORM",
        "reference-secret-architecture",
        "PATH_SECRET_PYTHON",
        "ENV_SECRET_PILLOW",
    ):
        assert secret not in report_one
    assert "predicted_boxes" not in report_one
    assert "reference_boxes" not in report_one
    assert "character_edits" not in report_one
    assert "prediction_present" not in report_one
    assert "PASS" in report_one
    assert "one table per page" in report_one
    assert "visible rules" in report_one
    assert "no merged cells" in report_one
    assert "no production or full-document authorization" in report_one


def test_public_report_api_rejects_private_canary_before_rendering():
    tuning = artifact_fixture("tuning")
    holdout = artifact_fixture("holdout")
    holdout["records"][0]["printable_private_field"] = "PRIVATE_DIRECT_CANARY"
    with (
        patch(
            "experiments.ruled_table._canonical_artifact_bindings",
            return_value=_canonical_bindings_fixture(),
        ),
        pytest.raises(ValueError, match="unknown keys"),
    ):
        render_report(tuning, holdout)


def test_tuning_without_winner_reports_stop_without_holdout():
    tuning = no_winner_tuning_artifact()
    with patch(
        "experiments.ruled_table._canonical_artifact_bindings",
        return_value=_canonical_bindings_fixture(),
    ):
        report = render_report(tuning)
        assert "## Decision: STOP" in report
        with pytest.raises(ValueError, match="must not include holdout"):
            render_report(tuning, artifact_fixture("holdout"))


def test_tuning_winner_requires_holdout_report_artifact():
    tuning = artifact_fixture("tuning")
    with (
        patch(
            "experiments.ruled_table._canonical_artifact_bindings",
            return_value=_canonical_bindings_fixture(),
        ),
        pytest.raises(ValueError, match="requires.*holdout"),
    ):
        render_report(tuning)


def test_holdout_run_fails_review_gate_before_opening_any_page(tmp_path):
    manifest_path = write_manifest_fixture(tmp_path)
    payload = json.loads(manifest_path.read_text(encoding="utf-8"))
    for page in payload["pages"]:
        if page["split"] == "holdout":
            page["review_status"] = "draft"
            page["reviewer"] = "pending-human-review"
            page["revision"] = 0
    _write_json(manifest_path, payload)
    with (
        patch("experiments.ruled_table.CorpusManifest.open_page") as opened,
        pytest.raises(ValueError, match="human_verified"),
    ):
        run_split(
            "holdout",
            manifest_path=manifest_path,
            tessdata=tmp_path / "never-opened",
            frozen_winner=artifact_fixture("tuning"),
        )
    opened.assert_not_called()


def test_holdout_cli_fails_before_opening_draft_canonical_holdout():
    manifest = SERVICE_ROOT / ".data" / "ruled-table" / "manifest.json"
    before = manifest.read_bytes()
    with (
        patch("experiments.ruled_table.CorpusManifest.open_page") as opened,
        pytest.raises(ValueError, match="human_verified"),
    ):
        main(
            [
                "holdout",
                    "--config",
                    str(CONFIG),
                "--manifest",
                str(manifest),
                "--annotations",
                str(manifest.parent / "annotations"),
                "--pdf",
                str(
                    SERVICE_ROOT
                    / ".data"
                    / "corpus"
                    / "official-89-2026-tt-btc.signed.pdf"
                ),
                "--tessdata",
                str(SERVICE_ROOT.parents[1] / "tessdata_best"),
                "--frozen-winner",
                "unused.json",
                "--output",
                "unused-output.json",
            ]
        )
    opened.assert_not_called()
    assert manifest.read_bytes() == before


def test_canonical_config_tracks_exact_vietnamese_tessdata_hash():
    config = load_config(CONFIG)
    assert config["tessdata"] == {
        "vie_sha256": "b6b49293d95d0b6dbd8780174627e82c75be957b6f4ed9862155540d6b00bb45"
    }


def test_failed_table_evidence_records_full_reference_deletions_without_text(
    tmp_path,
):
    path = tmp_path / "annotation.json"
    annotation_payload = valid_annotation()
    _write_json(path, annotation_payload)
    annotation = load_annotation(path, expected_render_sha256="b" * 64)
    evidence = _empty_cell_evidence(annotation)
    expected = [
        error_counts(cell["text"], "")
        for cell in annotation_payload["table"]["cells"]
    ]
    assert sum(item["character_edits"] for item in evidence) == sum(
        item.character_edits for item in expected
    )
    assert sum(item["reference_characters"] for item in evidence) == sum(
        item.reference_characters for item in expected
    )
    assert sum(item["word_edits"] for item in evidence) == sum(
        item.word_edits for item in expected
    )
    assert sum(item["reference_words"] for item in evidence) == sum(
        item.reference_words for item in expected
    )
    assert all(item["prediction_present"] is False for item in evidence)
    serialized = json.dumps(evidence, ensure_ascii=False)
    for cell in annotation_payload["table"]["cells"]:
        if cell["text"]:
            assert cell["text"] not in serialized


@pytest.mark.parametrize(
    ("candidate_id", "failed_pages", "expected_cer"),
    [
        ("strict-psm6", 5, 50 / 60),
        ("balanced-psm6", 4, 40 / 60),
    ],
)
def test_failure_pages_micro_average_full_reference_deletions(
    candidate_id, failed_pages, expected_cer
):
    payload = artifact_fixture("tuning")
    candidate_records = [
        record
        for record in payload["records"]
        if record["candidate_id"] == candidate_id
    ]
    for record in candidate_records[:failed_pages]:
        record["status"] = "invalid_grid"
        record["rows"] = 0
        record["columns"] = 0
        record["predicted_boxes"] = []
        record["resource"]["primary_error_kind"] = "invalid_grid"
        record["cells"][0].update(
            {
                "character_edits": 10,
                "reference_characters": 10,
                "word_edits": 2,
                "reference_words": 2,
                "predicted_blank": False,
                "prediction_present": False,
            }
        )
    recomputed = _recompute_aggregates(payload)
    aggregate = next(
        item for item in recomputed if item["candidate_id"] == candidate_id
    )
    assert aggregate["character_edits"] == failed_pages * 10
    assert aggregate["reference_characters"] == 60
    assert aggregate["word_edits"] == failed_pages * 2
    assert aggregate["reference_words"] == 12
    assert aggregate["cell_cer"] == pytest.approx(expected_cer)
    payload["aggregates"] = recomputed
    payload["winner_id"] = derive_tuning_winner(payload)
    with patch(
        "experiments.ruled_table._canonical_artifact_bindings",
        return_value=_canonical_bindings_fixture(),
    ):
        validate_artifact(payload, split="tuning")


def test_iou_ties_use_preserved_coordinates_not_input_order():
    references = [
        PositionedBox(0, 0, Box(0, 0, 10, 10)),
        PositionedBox(1, 0, Box(10, 0, 20, 10)),
    ]
    prediction_low = PositionedBox(0, 0, Box(20, 0, 30, 10))
    prediction_high = PositionedBox(0, 1, Box(30, 0, 40, 10))
    predictions = [prediction_high, prediction_low]

    def artificial_iou(reference, prediction):
        edges = {
            (0, 20): 0.8,
            (0, 30): 0.8,
            (10, 20): 0.8,
        }
        return edges.get((reference.left, prediction.left), 0.0)

    with patch("experiments.ruled_table._box_iou", artificial_iou):
        counts = match_cells(references, predictions, threshold=0.8)
    assert counts == CellMatchCounts(tp=1, fp=1, fn=1)


@pytest.mark.parametrize("field", ["predicted_boxes", "reference_boxes"])
def test_artifact_box_arrays_must_be_unique_and_row_major(field):
    payload = artifact_fixture()
    record = payload["records"][0]
    record["rows"] = 1
    record["columns"] = 2
    record["reference_rows"] = 1
    record["reference_columns"] = 2
    second_box = {"row": 0, "column": 1, "bbox": [100, 0, 200, 100]}
    record["predicted_boxes"].append(copy.deepcopy(second_box))
    record["reference_boxes"].append(copy.deepcopy(second_box))
    second_cell = copy.deepcopy(record["cells"][0])
    second_cell["column"] = 1
    record["cells"].append(second_cell)
    record[field].reverse()
    with (
        patch(
            "experiments.ruled_table._canonical_artifact_bindings",
            return_value=_canonical_bindings_fixture(),
        ),
        pytest.raises(ValueError, match="row-major"),
    ):
        validate_artifact(payload, split="holdout")


def _detected_grid_for_annotation(annotation: PageAnnotation) -> DetectionResult:
    table = annotation.table
    assert table is not None
    cells = tuple(
        GridCell(
            cell.row,
            cell.column,
            Box(*cell.bbox),
            Box(*cell.bbox),
        )
        for cell in table.cells
    )
    grid = Grid(
        table.rows,
        table.columns,
        Box(*table.bbox),
        Box(*table.bbox),
        cells,
    )
    return DetectionResult("detected", 0.0, (220, 130), grid, {})


@pytest.mark.parametrize(
    "primary_kind",
    ["timeout", "output_limit", "resource_limit", "candidate_error"],
)
def test_recognition_cleanup_failure_preserves_primary_and_cleanup(
    tmp_path, primary_kind
):
    annotation_path = tmp_path / "annotation.json"
    _write_json(annotation_path, valid_annotation())
    annotation = load_annotation(
        annotation_path, expected_render_sha256="b" * 64
    )
    manifest_page = ManifestPage(
        page_number=450,
        split="tuning",
        negative=False,
        template_family="fixture",
        template_fingerprint=template_fingerprint(annotation),
        render_path="private-render.png",
        render_sha256="c" * 64,
        annotation_path="private-annotation.json",
        annotation_sha256="f" * 64,
        review_status="human_verified",
        reviewer="reviewer@example.invalid",
        revision=1,
    )
    image = Image.new("L", (220, 130), 255)
    encoded = io.BytesIO()
    image.save(encoded, format="PNG")
    image.close()
    opened = OpenedPage(manifest_page, annotation, encoded.getvalue())
    failure = TableRecognitionError(primary_kind)
    failure.report_cleanup_failure(TableCleanupError())
    config = load_config(CONFIG)
    candidate = config["detector_candidates"][0]
    with (
        patch(
            "experiments.ruled_table.detect_ruled_table",
            return_value=_detected_grid_for_annotation(annotation),
        ),
        patch("experiments.ruled_table.recognize_grid", side_effect=failure),
    ):
        record = _run_page(
            opened,
            candidate=candidate,
            config=config,
            tessdata=tmp_path,
            bindings=_canonical_bindings_fixture(),
        )
    assert record["status"] == "cleanup_error"
    assert record["resource"]["cleanup_failed"] is True
    assert record["resource"]["primary_error_kind"] == primary_kind
    assert record["resource"]["timed_out"] is (primary_kind == "timeout")
    assert record["resource"]["output_limited"] is (
        primary_kind == "output_limit"
    )
    assert record["resource"]["candidate_failed"] is (
        primary_kind == "candidate_error"
    )
    assert record["resource"]["resource_violation"] is (
        primary_kind == "resource_limit"
    )
    assert record["rows"] == record["columns"] == 0
    assert record["predicted_boxes"] == []
    assert sum(item["character_edits"] for item in record["cells"]) == sum(
        item["reference_characters"] for item in record["cells"]
    )
    assert sum(item["word_edits"] for item in record["cells"]) == sum(
        item["reference_words"] for item in record["cells"]
    )
    aggregate = _aggregate_candidate(candidate["id"], [record])
    assert aggregate["cleanup_failures"] == 1
    assert aggregate["timeouts"] == (primary_kind == "timeout")
    assert aggregate["output_limit_failures"] == (
        primary_kind == "output_limit"
    )
    assert aggregate["candidate_failures"] == (
        primary_kind == "candidate_error"
    )
    assert aggregate["resource_violations"] == (
        primary_kind == "resource_limit"
    )


def test_validator_accepts_consistent_failure_and_rejects_retained_geometry():
    payload = artifact_fixture()
    record = payload["records"][0]
    record["status"] = "timeout"
    record["rows"] = 0
    record["columns"] = 0
    record["predicted_boxes"] = []
    record["resource"]["timed_out"] = True
    record["resource"]["primary_error_kind"] = "timeout"
    record["cells"][0].update(
        {
            "character_edits": 10,
            "reference_characters": 10,
            "word_edits": 2,
            "reference_words": 2,
            "prediction_present": False,
        }
    )
    payload["aggregates"] = _recompute_aggregates(payload)
    payload["decision"] = "STOP"
    with patch(
        "experiments.ruled_table._canonical_artifact_bindings",
        return_value=_canonical_bindings_fixture(),
    ):
        validate_artifact(payload, split="holdout")

    record["rows"] = 1
    record["columns"] = 1
    record["predicted_boxes"] = [
        {"row": 0, "column": 0, "bbox": [0, 0, 100, 100]}
    ]
    with (
        patch(
            "experiments.ruled_table._canonical_artifact_bindings",
            return_value=_canonical_bindings_fixture(),
        ),
        pytest.raises(ValueError, match="non-detected.*grid"),
    ):
        validate_artifact(payload, split="holdout")


def test_task_five_cli_exposes_exact_planned_arguments():
    parser = _parser()
    tune = parser.parse_args(
        [
            "tune",
            "--config",
            "config.json",
            "--manifest",
            "manifest.json",
            "--annotations",
            "annotations",
            "--pdf",
            "source.pdf",
            "--tessdata",
            "tessdata",
            "--output",
            "tuning.json",
        ]
    )
    assert tune.annotations == Path("annotations")
    assert tune.pdf == Path("source.pdf")
    holdout = parser.parse_args(
        [
            "holdout",
            "--config",
            "config.json",
            "--frozen-winner",
            "winner.json",
            "--manifest",
            "manifest.json",
            "--annotations",
            "annotations",
            "--pdf",
            "source.pdf",
            "--tessdata",
            "tessdata",
            "--output",
            "holdout.json",
        ]
    )
    assert holdout.frozen_winner == Path("winner.json")
    validation = parser.parse_args(
        ["validate", "--input", "tuning.json", "--split", "tuning"]
    )
    assert validation.input == Path("tuning.json")
    report = parser.parse_args(
        [
            "report",
            "--tuning",
            "tuning.json",
            "--holdout",
            "holdout.json",
            "--output",
            "report.md",
        ]
    )
    assert report.tuning == Path("tuning.json")
    assert report.holdout == Path("holdout.json")
    tuning_only_report = parser.parse_args(
        [
            "report",
            "--tuning",
            "tuning.json",
            "--output",
            "report.md",
        ]
    )
    assert tuning_only_report.holdout is None


def test_inventory_require_review_rejects_before_page_open(tmp_path):
    manifest = SERVICE_ROOT / ".data" / "ruled-table" / "manifest.json"
    output = tmp_path / "inventory.md"
    with (
        patch("experiments.ruled_table.CorpusManifest.open_page") as opened,
        pytest.raises(ValueError, match="12/12.*human_verified|0/12"),
    ):
        main(
            [
                "inventory",
                "--manifest",
                str(manifest),
                "--annotations",
                str(manifest.parent / "annotations"),
                "--output",
                str(output),
                "--require-human-review",
            ]
        )
    opened.assert_not_called()
    assert not output.exists()


@pytest.mark.parametrize("kind", ["config", "manifest", "pdf", "tessdata"])
def test_altered_official_input_fails_before_page_open(tmp_path, kind):
    config = tmp_path / "config.json"
    config.write_bytes(CONFIG.read_bytes())
    manifest = tmp_path / "manifest.json"
    canonical_manifest = SERVICE_ROOT / ".data" / "ruled-table" / "manifest.json"
    manifest.write_bytes(canonical_manifest.read_bytes())
    pdf = tmp_path / "source.pdf"
    canonical_pdf = (
        SERVICE_ROOT / ".data" / "corpus" / "official-89-2026-tt-btc.signed.pdf"
    )
    shutil.copyfile(canonical_pdf, pdf)
    tessdata = tmp_path / "tessdata"
    tessdata.mkdir()
    shutil.copyfile(
        SERVICE_ROOT.parents[1] / "tessdata_best" / "vie.traineddata",
        tessdata / "vie.traineddata",
    )
    target = {
        "config": config,
        "manifest": manifest,
        "pdf": pdf,
        "tessdata": tessdata / "vie.traineddata",
    }[kind]
    with target.open("ab") as stream:
        stream.write(b"altered")
    with (
        patch(
            "experiments.ruled_table._verify_annotation_readiness",
            return_value=12,
        ),
        patch("experiments.ruled_table.CorpusManifest.open_page") as opened,
        pytest.raises(ValueError),
    ):
        run_split(
            "tuning",
            config_path=config,
            manifest_path=manifest,
            annotations_path=manifest.parent / "annotations",
            pdf_path=pdf,
            tessdata=tessdata,
        )
    opened.assert_not_called()


def test_byte_identical_custom_inputs_pass_preflight_before_open(tmp_path):
    config = tmp_path / "config.json"
    config.write_bytes(CONFIG.read_bytes())
    manifest = tmp_path / "manifest.json"
    canonical_manifest = SERVICE_ROOT / ".data" / "ruled-table" / "manifest.json"
    manifest.write_bytes(canonical_manifest.read_bytes())
    pdf = tmp_path / "source.pdf"
    shutil.copyfile(
        SERVICE_ROOT / ".data" / "corpus" / "official-89-2026-tt-btc.signed.pdf",
        pdf,
    )
    tessdata = tmp_path / "tessdata"
    tessdata.mkdir()
    shutil.copyfile(
        SERVICE_ROOT.parents[1] / "tessdata_best" / "vie.traineddata",
        tessdata / "vie.traineddata",
    )
    with (
        patch(
            "experiments.ruled_table._verify_annotation_readiness",
            return_value=12,
        ),
        patch(
            "experiments.ruled_table.CorpusManifest.open_page",
            side_effect=RuntimeError("preflight passed"),
        ) as opened,
        pytest.raises(RuntimeError, match="preflight passed"),
    ):
        run_split(
            "tuning",
            config_path=config,
            manifest_path=manifest,
            annotations_path=manifest.parent / "annotations",
            pdf_path=pdf,
            tessdata=tessdata,
        )
    assert opened.call_count == 1


def test_frozen_winner_binds_exact_validated_tuning_bytes(tmp_path):
    tuning = artifact_fixture("tuning")
    tuning_path = tmp_path / "raw" / "tuning.json"
    _write_json(tuning_path, tuning)
    with patch(
        "experiments.ruled_table._canonical_artifact_bindings",
        return_value=_canonical_bindings_fixture(),
    ):
        frozen = freeze_tuning_winner(
            tuning,
            tuning_artifact_bytes=tuning_path.read_bytes(),
        )
        assert set(frozen) == {
            "schema_version",
            "winner_id",
            "configuration_sha256",
            "config_sha256",
            "manifest_sha256",
            "tuning_artifact_sha256",
        }
        validate_frozen_winner(
            frozen,
            tuning_artifact_path=tuning_path,
        )
    assert frozen["tuning_artifact_sha256"] == hashlib.sha256(
        tuning_path.read_bytes()
    ).hexdigest()

    tuning_path.write_bytes(tuning_path.read_bytes() + b" ")
    with (
        patch(
            "experiments.ruled_table._canonical_artifact_bindings",
            return_value=_canonical_bindings_fixture(),
        ),
        pytest.raises(ValueError, match="tuning artifact.*SHA-256"),
    ):
        validate_frozen_winner(
            frozen,
            tuning_artifact_path=tuning_path,
        )


def test_frozen_winner_rejects_fabricated_candidate_against_tuning(tmp_path):
    tuning = artifact_fixture("tuning")
    tuning_path = tmp_path / "raw" / "tuning.json"
    _write_json(tuning_path, tuning)
    with patch(
        "experiments.ruled_table._canonical_artifact_bindings",
        return_value=_canonical_bindings_fixture(),
    ):
        frozen = freeze_tuning_winner(
            tuning,
            tuning_artifact_bytes=tuning_path.read_bytes(),
        )
    frozen["configuration_sha256"] = "f" * 64
    with (
        patch(
            "experiments.ruled_table._canonical_artifact_bindings",
            return_value=_canonical_bindings_fixture(),
        ),
        pytest.raises(ValueError, match="configuration"),
    ):
        validate_frozen_winner(
            frozen,
            tuning_artifact_path=tuning_path,
        )


def _valid_frozen_files(tmp_path: Path) -> tuple[Path, dict]:
    tuning = artifact_fixture("tuning")
    tuning_path = tmp_path / "raw" / "tuning.json"
    _write_json(tuning_path, tuning)
    with patch(
        "experiments.ruled_table._canonical_artifact_bindings",
        return_value=_canonical_bindings_fixture(),
    ):
        frozen = freeze_tuning_winner(
            tuning,
            tuning_artifact_bytes=tuning_path.read_bytes(),
        )
    return tuning_path, frozen


def test_holdout_marker_is_atomic_and_blocks_crashed_rerun_before_access(
    tmp_path,
):
    tuning_path, frozen = _valid_frozen_files(tmp_path)
    marker = tuning_path.parent / "holdout.started.json"
    manifest = load_manifest(
        SERVICE_ROOT / ".data" / "ruled-table" / "manifest.json",
        mode="holdout",
    )
    config = load_config(CONFIG)
    with (
        patch(
            "experiments.ruled_table._preflight_official_inputs",
            return_value=(config, manifest),
        ),
        patch(
            "experiments.ruled_table._canonical_artifact_bindings",
            return_value=_canonical_bindings_fixture(),
        ),
        patch(
            "experiments.ruled_table.CorpusManifest.open_page",
            side_effect=RuntimeError("simulated crash after marker"),
        ) as opened,
        pytest.raises(RuntimeError, match="simulated crash"),
    ):
        run_split(
            "holdout",
            frozen_winner=frozen,
            tuning_artifact_path=tuning_path,
            holdout_marker_path=marker,
        )
    assert opened.call_count == 1
    marker_payload = json.loads(marker.read_bytes())
    assert set(marker_payload) == {
        "schema_version",
        "split",
        "config_sha256",
        "manifest_sha256",
        "tuning_artifact_sha256",
        "winner_id",
        "holdout_pages",
        "negative_pages",
    }
    assert "path" not in json.dumps(marker_payload)

    with (
        patch(
            "experiments.ruled_table._preflight_official_inputs",
            return_value=(config, manifest),
        ),
        patch(
            "experiments.ruled_table._canonical_artifact_bindings",
            return_value=_canonical_bindings_fixture(),
        ),
        patch("experiments.ruled_table.CorpusManifest.open_page") as opened_again,
        pytest.raises(ValueError, match="already.*attempted|marker"),
    ):
        run_split(
            "holdout",
            frozen_winner=frozen,
            tuning_artifact_path=tuning_path,
            holdout_marker_path=marker,
        )
    opened_again.assert_not_called()
    assert manifest.access_counts == {
        "tuning": 0,
        "holdout": 0,
        "negative": 0,
    }


@pytest.mark.parametrize("fabrication", ["hash", "candidate"])
def test_fabricated_frozen_winner_fails_before_marker_or_page(
    tmp_path, fabrication
):
    tuning_path, frozen = _valid_frozen_files(tmp_path)
    if fabrication == "hash":
        frozen["tuning_artifact_sha256"] = "f" * 64
    else:
        strict = _candidate_descriptors()[0]
        frozen["winner_id"] = strict["id"]
        frozen["configuration_sha256"] = strict["configuration_sha256"]
    marker = tuning_path.parent / "holdout.started.json"
    manifest = load_manifest(
        SERVICE_ROOT / ".data" / "ruled-table" / "manifest.json",
        mode="holdout",
    )
    config = load_config(CONFIG)
    with (
        patch(
            "experiments.ruled_table._preflight_official_inputs",
            return_value=(config, manifest),
        ),
        patch(
            "experiments.ruled_table._canonical_artifact_bindings",
            return_value=_canonical_bindings_fixture(),
        ),
        patch("experiments.ruled_table.CorpusManifest.open_page") as opened,
        pytest.raises(ValueError, match="tuning artifact|winner"),
    ):
        run_split(
            "holdout",
            frozen_winner=frozen,
            tuning_artifact_path=tuning_path,
            holdout_marker_path=marker,
        )
    opened.assert_not_called()
    assert not marker.exists()
    assert manifest.access_counts == {
        "tuning": 0,
        "holdout": 0,
        "negative": 0,
    }


@pytest.mark.parametrize("kind", ["stale", "noncanonical", "private"])
def test_report_cli_validates_every_artifact_before_rendering(
    tmp_path, kind
):
    tuning = artifact_fixture("tuning")
    holdout = artifact_fixture("holdout")
    if kind == "stale":
        holdout["aggregates"][0]["cell_tp"] += 1
    elif kind == "noncanonical":
        tuning["config_sha256"] = "f" * 64
    else:
        holdout["records"][0]["printable_private_field"] = "PRIVATE_CANARY"
    tuning_path = tmp_path / "tuning.json"
    holdout_path = tmp_path / "holdout.json"
    output = tmp_path / "report.md"
    _write_json(tuning_path, tuning)
    _write_json(holdout_path, holdout)
    with (
        patch(
            "experiments.ruled_table._canonical_artifact_bindings",
            return_value=_canonical_bindings_fixture(),
        ),
        patch("experiments.ruled_table.render_report") as renderer,
        pytest.raises(ValueError),
    ):
        main(
            [
                "report",
                "--tuning",
                str(tuning_path),
                "--holdout",
                str(holdout_path),
                "--output",
                str(output),
            ]
        )
    renderer.assert_not_called()
    assert not output.exists()
