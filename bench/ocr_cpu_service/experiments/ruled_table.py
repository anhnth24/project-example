#!/usr/bin/env python3
"""Frozen corpus contracts for the benchmark-only ruled-table OCR spike."""

from __future__ import annotations

import argparse
import hashlib
import importlib.metadata
import io
import json
import math
import os
import platform
import re
import shutil
import statistics
import subprocess
import tempfile
import time
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path, PurePosixPath
from typing import Any, Literal, Mapping, Sequence

import psutil
from PIL import Image

from benchmark.metrics import error_counts, normalize_for_metric
from benchmark.render import RenderLimits, open_pdf, render_page
from experiments.table_cells import (
    GridRecognition,
    ProcessLimits,
    TableRecognitionError,
    recognize_grid,
)
from experiments.table_lines import (
    Box,
    DetectorConfig,
    detect_ruled_table,
    prepare_working_image,
)


SERVICE_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CONFIG = SERVICE_ROOT / "experiments" / "ruled-table-config.json"
DEFAULT_MANIFEST = SERVICE_ROOT / ".data" / "ruled-table" / "manifest.json"
DEFAULT_ANNOTATIONS = SERVICE_ROOT / ".data" / "ruled-table" / "annotations"
DEFAULT_PDF = (
    SERVICE_ROOT
    / ".data"
    / "corpus"
    / "official-89-2026-tt-btc.signed.pdf"
)
DEFAULT_TESSDATA = SERVICE_ROOT.parents[1] / "tessdata_best"
DEFAULT_RAW_ARTIFACT = SERVICE_ROOT / ".data" / "ruled-table" / "raw-tuning.json"
DEFAULT_REPORT = SERVICE_ROOT / "reports" / "ruled-table-spike.md"
CANONICAL_CONFIG_SHA256 = (
    "53882ed34ec756fd2fc9e7bb3ad66ac021c86b26c70a04299f8dd1b1eec0a3f8"
)
CANONICAL_MANIFEST_SHA256 = (
    "89511ce0a181be774582075730b502c97170f3166b35acae9a0ca3eb475df6a4"
)
CANONICAL_SOURCE_ID = "official-89-2026-tt-btc"
CANONICAL_SOURCE_SHA256 = (
    "952c45ffc0f10bfc176bd9ae6b3d204fd3a034294ee270278957b9c11e1471dc"
)
CANONICAL_SOURCE_SIZE = 17_281_751

_SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
_CONFIG_FIELDS = frozenset(
    {
        "schema_version",
        "source",
        "render",
        "tessdata",
        "geometry_limits",
        "process_limits",
        "detector_candidates",
        "gate",
    }
)
_SOURCE_CONFIG_FIELDS = frozenset({"id", "expected_sha256", "max_bytes"})
_RENDER_FIELDS = frozenset({"dpi", "max_pixels", "max_dimension"})
_TESSDATA_FIELDS = frozenset({"vie_sha256"})
_GEOMETRY_FIELDS = frozenset(
    {
        "max_rows",
        "max_columns",
        "max_cells",
        "max_table_regions",
        "cell_match_iou",
    }
)
_PROCESS_FIELDS = frozenset(
    {
        "cpu_threads",
        "page_timeout_seconds",
        "cell_timeout_seconds",
        "max_output_bytes_per_cell",
        "max_output_bytes_per_page",
        "max_rss_bytes",
        "sample_interval_ms",
    }
)
_CANDIDATE_FIELDS = frozenset(
    {
        "id",
        "dark_max",
        "min_horizontal_fraction",
        "min_vertical_fraction",
        "max_gap_pixels",
        "cluster_tolerance_pixels",
        "intersection_tolerance_pixels",
        "deskew_angles_degrees",
        "cell_inset_pixels",
        "psm",
    }
)
_GATE_FIELDS = frozenset(
    {
        "exact_grid_required",
        "minimum_cell_f1",
        "maximum_cell_cer",
        "minimum_empty_cell_accuracy",
        "maximum_negative_false_positives",
    }
)
_ANNOTATION_FIELDS = frozenset(
    {
        "schema_version",
        "source_sha256",
        "render_sha256",
        "page_number",
        "split",
        "negative",
        "review",
        "table",
    }
)
_REVIEW_FIELDS = frozenset(
    {"review_status", "reviewer", "revision", "reviewed_at"}
)
_TABLE_FIELDS = frozenset({"bbox", "rows", "columns", "cells"})
_CELL_FIELDS = frozenset({"row", "column", "bbox", "text", "blank"})
_MANIFEST_FIELDS = frozenset({"schema_version", "source", "pages"})
_MANIFEST_SOURCE_FIELDS = frozenset({"id", "sha256", "size_bytes"})
_MANIFEST_PAGE_FIELDS = frozenset(
    {
        "page_number",
        "split",
        "negative",
        "template_family",
        "template_fingerprint",
        "render_path",
        "render_sha256",
        "annotation_path",
        "annotation_sha256",
        "review_status",
        "reviewer",
        "revision",
    }
)
_SELECTION_FIELDS = frozenset(
    {"page_number", "split", "negative", "template_family"}
)
_EXPECTED_CANDIDATE_IDS = (
    "strict-psm6",
    "balanced-psm6",
    "balanced-psm7",
)
_EXPECTED_PSMS = {
    "strict-psm6": 6,
    "balanced-psm6": 6,
    "balanced-psm7": 7,
}
_EXPECTED_DESKEW_ANGLES = (-1.5, -1.0, -0.5, 0.0, 0.5, 1.0, 1.5)


@dataclass(frozen=True, slots=True)
class CellReference:
    row: int
    column: int
    bbox: tuple[int, int, int, int]
    text: str
    blank: bool


@dataclass(frozen=True, slots=True)
class TableReference:
    bbox: tuple[int, int, int, int]
    rows: int
    columns: int
    cells: tuple[CellReference, ...]


@dataclass(frozen=True, slots=True)
class PageAnnotation:
    page_number: int
    split: str
    negative: bool
    source_sha256: str
    render_sha256: str
    review_status: str
    reviewer: str
    revision: int
    table: TableReference | None


@dataclass(frozen=True, slots=True)
class ManifestPage:
    page_number: int
    split: str
    negative: bool
    template_family: str
    template_fingerprint: str | None
    render_path: str
    render_sha256: str
    annotation_path: str
    annotation_sha256: str
    review_status: str
    reviewer: str
    revision: int


@dataclass(frozen=True, slots=True)
class OpenedPage:
    page: ManifestPage
    annotation: PageAnnotation
    render_bytes: bytes = field(repr=False)


@dataclass(frozen=True, slots=True)
class RenderedPage:
    page_number: int
    split: str
    negative: bool
    template_family: str
    path: Path
    sha256: str
    width: int
    height: int


@dataclass(frozen=True, slots=True)
class CorpusManifest:
    source_id: str
    source_sha256: str
    source_size_bytes: int
    tuning: tuple[ManifestPage, ...]
    holdout: tuple[ManifestPage, ...]
    negative: tuple[ManifestPage, ...]
    manifest_sha256: str
    _root: Path = field(repr=False, compare=False)
    _mode: Literal["tuning", "holdout"] = field(repr=False, compare=False)
    _access_counts: dict[str, int] = field(
        default_factory=lambda: {"tuning": 0, "holdout": 0, "negative": 0},
        repr=False,
        compare=False,
    )

    @property
    def access_counts(self) -> dict[str, int]:
        return dict(self._access_counts)

    def open_page(self, page_number: int) -> OpenedPage:
        if not _is_int(page_number) or page_number <= 0:
            raise ValueError("page_number must be a positive integer")
        by_number = {
            page.page_number: page
            for page in self.tuning + self.holdout + self.negative
        }
        try:
            page = by_number[page_number]
        except KeyError as error:
            raise ValueError(f"page {page_number} is not frozen") from error

        category = (
            "negative"
            if page.negative
            else "tuning"
            if page.split == "tuning"
            else "holdout"
        )
        if self._mode == "tuning" and page.split == "holdout":
            raise PermissionError("holdout access denied in tuning mode")
        if self._mode == "holdout" and page.split == "tuning":
            raise PermissionError("tuning access denied in holdout mode")

        render_path = _resolve_private_path(self._root, page.render_path)
        annotation_path = _resolve_private_path(self._root, page.annotation_path)
        render_bytes = render_path.read_bytes()
        if hashlib.sha256(render_bytes).hexdigest() != page.render_sha256:
            raise ValueError("render SHA-256 mismatch")
        annotation_bytes = annotation_path.read_bytes()
        if hashlib.sha256(annotation_bytes).hexdigest() != page.annotation_sha256:
            raise ValueError("annotation SHA-256 mismatch")
        annotation = load_annotation(
            annotation_path,
            expected_render_sha256=page.render_sha256,
        )
        if template_fingerprint(annotation) != page.template_fingerprint:
            raise ValueError("template fingerprint mismatch")
        if (
            annotation.source_sha256 != self.source_sha256
            or annotation.page_number != page.page_number
            or annotation.split != page.split
            or annotation.negative != page.negative
            or annotation.review_status != page.review_status
            or annotation.reviewer != page.reviewer
            or annotation.revision != page.revision
        ):
            raise ValueError("annotation metadata does not match manifest")
        self._access_counts[category] += 1
        return OpenedPage(page=page, annotation=annotation, render_bytes=render_bytes)


def _closed_mapping(
    value: object,
    *,
    fields: frozenset[str],
    name: str,
) -> Mapping[str, Any]:
    if not isinstance(value, dict):
        raise ValueError(f"{name} must be an object")
    keys = set(value)
    unknown = keys - fields
    missing = fields - keys
    if unknown:
        raise ValueError(f"{name} has unknown keys: {sorted(unknown)}")
    if missing:
        raise ValueError(f"{name} has missing keys: {sorted(missing)}")
    return value


def _is_int(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def _positive_int(value: object, name: str) -> int:
    if not _is_int(value) or value <= 0:
        raise ValueError(f"{name} must be a positive integer")
    return value


def _nonnegative_int(value: object, name: str) -> int:
    if not _is_int(value) or value < 0:
        raise ValueError(f"{name} must be a nonnegative integer")
    return value


def _number(value: object, name: str) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError(f"{name} must be a number")
    return float(value)


def _fraction(value: object, name: str, *, zero_allowed: bool = True) -> float:
    number = _number(value, name)
    lower_ok = number >= 0 if zero_allowed else number > 0
    if not lower_ok or number > 1:
        raise ValueError(f"{name} must be between 0 and 1")
    return number


def _string(value: object, name: str, *, nonempty: bool = True) -> str:
    if not isinstance(value, str) or (nonempty and not value.strip()):
        raise ValueError(f"{name} must be a nonempty string")
    return value


def _sha256(value: object, name: str) -> str:
    text = _string(value, name)
    if _SHA256_RE.fullmatch(text) is None:
        raise ValueError(f"{name} must be a lowercase SHA-256")
    return text


def _bool(value: object, name: str) -> bool:
    if not isinstance(value, bool):
        raise ValueError(f"{name} must be a boolean")
    return value


def _bbox(value: object, name: str) -> tuple[int, int, int, int]:
    if (
        not isinstance(value, list)
        or len(value) != 4
        or any(not _is_int(item) for item in value)
    ):
        raise ValueError(f"{name} must contain four integers")
    x1, y1, x2, y2 = value
    if x1 < 0 or y1 < 0 or x2 <= x1 or y2 <= y1:
        raise ValueError(f"{name} must have positive area and nonnegative origin")
    return (x1, y1, x2, y2)


def _validate_review(
    value: object,
    *,
    name: str,
) -> tuple[str, str, int]:
    review = _closed_mapping(value, fields=_REVIEW_FIELDS, name=name)
    status = _string(review["review_status"], f"{name}.review_status")
    if status not in {"draft", "human_verified"}:
        raise ValueError(f"{name}.review_status is unsupported")
    reviewer = _string(review["reviewer"], f"{name}.reviewer")
    revision = _nonnegative_int(review["revision"], f"{name}.revision")
    reviewed_at = review["reviewed_at"]
    if status == "human_verified":
        if revision < 1:
            raise ValueError(f"{name}.revision must be positive after human review")
        timestamp = _string(reviewed_at, f"{name}.reviewed_at")
        if not timestamp.endswith("Z"):
            raise ValueError(f"{name}.reviewed_at must be a UTC timestamp")
        try:
            datetime.fromisoformat(timestamp.removesuffix("Z") + "+00:00")
        except ValueError as error:
            raise ValueError(f"{name}.reviewed_at must be a UTC timestamp") from error
    elif reviewed_at is not None:
        raise ValueError(f"{name}.reviewed_at must be null while draft")
    return status, reviewer, revision


def _validate_config(payload: object) -> dict[str, Any]:
    config = _closed_mapping(payload, fields=_CONFIG_FIELDS, name="config")
    if config["schema_version"] != 1 or not _is_int(config["schema_version"]):
        raise ValueError("config.schema_version must equal 1")

    source = _closed_mapping(
        config["source"], fields=_SOURCE_CONFIG_FIELDS, name="config.source"
    )
    if source["id"] != CANONICAL_SOURCE_ID:
        raise ValueError("config.source.id is not the canonical source identity")
    if _sha256(source["expected_sha256"], "config.source.expected_sha256") != (
        CANONICAL_SOURCE_SHA256
    ):
        raise ValueError("config.source.expected_sha256 is not canonical")
    if (
        _positive_int(source["max_bytes"], "config.source.max_bytes")
        > CANONICAL_SOURCE_SIZE
    ):
        raise ValueError("config.source.max_bytes exceeds the canonical limit")

    render = _closed_mapping(
        config["render"], fields=_RENDER_FIELDS, name="config.render"
    )
    dpi = _positive_int(render["dpi"], "config.render.dpi")
    if dpi > 300:
        raise ValueError("config.render.dpi exceeds the safe range")
    if _positive_int(render["max_pixels"], "config.render.max_pixels") > 50_000_000:
        raise ValueError("config.render.max_pixels exceeds the canonical limit")
    if _positive_int(render["max_dimension"], "config.render.max_dimension") > 10_000:
        raise ValueError("config.render.max_dimension exceeds the canonical limit")

    tessdata = _closed_mapping(
        config["tessdata"], fields=_TESSDATA_FIELDS, name="config.tessdata"
    )
    _sha256(tessdata["vie_sha256"], "config.tessdata.vie_sha256")

    geometry = _closed_mapping(
        config["geometry_limits"],
        fields=_GEOMETRY_FIELDS,
        name="config.geometry_limits",
    )
    rows = _positive_int(geometry["max_rows"], "geometry_limits.max_rows")
    columns = _positive_int(geometry["max_columns"], "geometry_limits.max_columns")
    cells = _positive_int(geometry["max_cells"], "geometry_limits.max_cells")
    if rows > 50:
        raise ValueError("geometry_limits.max_rows exceeds 50")
    if columns > 30:
        raise ValueError("geometry_limits.max_columns exceeds 30")
    if cells > 1_500:
        raise ValueError("geometry_limits.max_cells exceeds 1500")
    if rows * columns > cells:
        raise ValueError(
            "canonical geometry semantics require max_rows * max_columns "
            "not to exceed max_cells"
        )
    if (
        _positive_int(
            geometry["max_table_regions"], "geometry_limits.max_table_regions"
        )
        != 1
    ):
        raise ValueError(
            "canonical geometry semantics require max_table_regions to equal 1"
        )
    _fraction(
        geometry["cell_match_iou"],
        "geometry_limits.cell_match_iou",
        zero_allowed=False,
    )

    process = _closed_mapping(
        config["process_limits"],
        fields=_PROCESS_FIELDS,
        name="config.process_limits",
    )
    process_caps = {
        "cpu_threads": 1,
        "page_timeout_seconds": 20,
        "cell_timeout_seconds": 10,
        "max_output_bytes_per_cell": 65_536,
        "max_output_bytes_per_page": 1_048_576,
        "max_rss_bytes": 805_306_368,
        "sample_interval_ms": 10,
    }
    for key, cap in process_caps.items():
        if _positive_int(process[key], f"process_limits.{key}") > cap:
            raise ValueError(f"process_limits.{key} exceeds the canonical limit")
    if process["cpu_threads"] != 1:
        raise ValueError("process_limits.cpu_threads must equal 1")

    candidates = config["detector_candidates"]
    if not isinstance(candidates, list):
        raise ValueError("config.detector_candidates must be an array")
    candidate_ids: list[str] = []
    for index, candidate_value in enumerate(candidates):
        candidate = _closed_mapping(
            candidate_value,
            fields=_CANDIDATE_FIELDS,
            name=f"config.detector_candidates[{index}]",
        )
        candidate_id = _string(
            candidate["id"], f"detector_candidates[{index}].id"
        )
        candidate_ids.append(candidate_id)
        dark_max = _positive_int(
            candidate["dark_max"], f"detector_candidates[{index}].dark_max"
        )
        if dark_max > 255:
            raise ValueError(f"detector_candidates[{index}].dark_max is invalid")
        _fraction(
            candidate["min_horizontal_fraction"],
            f"detector_candidates[{index}].min_horizontal_fraction",
            zero_allowed=False,
        )
        _fraction(
            candidate["min_vertical_fraction"],
            f"detector_candidates[{index}].min_vertical_fraction",
            zero_allowed=False,
        )
        for key in (
            "max_gap_pixels",
            "cluster_tolerance_pixels",
            "intersection_tolerance_pixels",
            "cell_inset_pixels",
        ):
            _nonnegative_int(candidate[key], f"detector_candidates[{index}].{key}")
        angles = candidate["deskew_angles_degrees"]
        if (
            not isinstance(angles, list)
            or any(
                isinstance(angle, bool) or not isinstance(angle, (int, float))
                for angle in angles
            )
            or tuple(float(angle) for angle in angles) != _EXPECTED_DESKEW_ANGLES
        ):
            raise ValueError("candidate semantics require fixed deskew angles")
        psm = _positive_int(
            candidate["psm"], f"detector_candidates[{index}].psm"
        )
        if _EXPECTED_PSMS.get(candidate_id) != psm:
            raise ValueError("candidate semantics require the fixed PSM")
    if tuple(candidate_ids) != _EXPECTED_CANDIDATE_IDS:
        raise ValueError("detector candidate IDs are not canonical")

    gate = _closed_mapping(config["gate"], fields=_GATE_FIELDS, name="config.gate")
    if _bool(gate["exact_grid_required"], "gate.exact_grid_required") is not True:
        raise ValueError(
            "canonical gate semantics require exact_grid_required to be true"
        )
    _fraction(
        gate["minimum_cell_f1"], "gate.minimum_cell_f1", zero_allowed=False
    )
    _fraction(gate["maximum_cell_cer"], "gate.maximum_cell_cer")
    _fraction(
        gate["minimum_empty_cell_accuracy"],
        "gate.minimum_empty_cell_accuracy",
        zero_allowed=False,
    )
    _nonnegative_int(
        gate["maximum_negative_false_positives"],
        "gate.maximum_negative_false_positives",
    )
    canonical_payload = json.loads(DEFAULT_CONFIG.read_bytes())
    for section in (
        "source",
        "render",
        "tessdata",
        "geometry_limits",
        "process_limits",
        "detector_candidates",
        "gate",
    ):
        if config[section] != canonical_payload[section]:
            raise ValueError(f"config.{section} must match canonical semantics exactly")
    return dict(config)


def load_config(path: Path) -> dict[str, Any]:
    """Load a closed, bounded detector configuration."""
    raw = path.read_bytes()
    try:
        payload = json.loads(raw)
    except (json.JSONDecodeError, UnicodeDecodeError) as error:
        raise ValueError("config is not valid UTF-8 JSON") from error
    config = _validate_config(payload)
    actual_sha256 = hashlib.sha256(raw).hexdigest()
    if actual_sha256 != CANONICAL_CONFIG_SHA256:
        raise ValueError("canonical config SHA-256 mismatch")
    return config


def process_limits(path: Path = DEFAULT_CONFIG) -> ProcessLimits:
    """Build typed process bounds from the validated canonical config."""
    config = load_config(path)
    return ProcessLimits(**config["process_limits"])


def detector_config(
    config: Mapping[str, Any], candidate_id: str
) -> DetectorConfig:
    """Build one typed geometry candidate from validated canonical config."""
    candidates = config.get("detector_candidates")
    geometry = config.get("geometry_limits")
    render = config.get("render")
    if (
        not isinstance(candidates, list)
        or not isinstance(geometry, Mapping)
        or not isinstance(render, Mapping)
    ):
        raise ValueError("config must contain validated detector geometry")
    matches = [
        candidate
        for candidate in candidates
        if isinstance(candidate, Mapping) and candidate.get("id") == candidate_id
    ]
    if len(matches) != 1:
        raise ValueError("candidate_id must identify one canonical candidate")
    candidate = matches[0]
    return DetectorConfig(
        dark_max=candidate["dark_max"],
        min_horizontal_fraction=candidate["min_horizontal_fraction"],
        min_vertical_fraction=candidate["min_vertical_fraction"],
        max_gap_pixels=candidate["max_gap_pixels"],
        cluster_tolerance_pixels=candidate["cluster_tolerance_pixels"],
        intersection_tolerance_pixels=candidate[
            "intersection_tolerance_pixels"
        ],
        deskew_angles_degrees=tuple(candidate["deskew_angles_degrees"]),
        max_rows=geometry["max_rows"],
        max_columns=geometry["max_columns"],
        max_cells=geometry["max_cells"],
        max_pixels=render["max_pixels"],
        max_dimension=render["max_dimension"],
    )


def _parse_annotation(
    payload: object,
    *,
    expected_render_sha256: str,
    require_verified_holdout: bool,
) -> PageAnnotation:
    annotation = _closed_mapping(
        payload, fields=_ANNOTATION_FIELDS, name="annotation"
    )
    if (
        annotation["schema_version"] != 1
        or not _is_int(annotation["schema_version"])
    ):
        raise ValueError("annotation.schema_version must equal 1")
    source_sha256 = _sha256(annotation["source_sha256"], "annotation.source_sha256")
    render_sha256 = _sha256(annotation["render_sha256"], "annotation.render_sha256")
    if render_sha256 != _sha256(
        expected_render_sha256, "expected_render_sha256"
    ):
        raise ValueError("render SHA-256 mismatch")
    page_number = _positive_int(annotation["page_number"], "annotation.page_number")
    split = _string(annotation["split"], "annotation.split")
    if split not in {"tuning", "holdout"}:
        raise ValueError("annotation.split must be tuning or holdout")
    negative = _bool(annotation["negative"], "annotation.negative")
    review_status, reviewer, revision = _validate_review(
        annotation["review"], name="annotation.review"
    )

    table_value = annotation["table"]
    if negative:
        if table_value is not None:
            raise ValueError("negative annotation table must be null")
        table = None
    else:
        table_payload = _closed_mapping(
            table_value, fields=_TABLE_FIELDS, name="annotation.table"
        )
        table_bbox = _bbox(table_payload["bbox"], "annotation.table.bbox")
        rows = _positive_int(table_payload["rows"], "annotation.table.rows")
        columns = _positive_int(
            table_payload["columns"], "annotation.table.columns"
        )
        if rows > 50:
            raise ValueError("annotation.table.max_rows limit is 50")
        if columns > 30:
            raise ValueError("annotation.table.max_columns limit is 30")
        cell_count = rows * columns
        if cell_count > 1_500:
            raise ValueError("annotation table exceeds maximum 1500 cells")
        cells_value = table_payload["cells"]
        if not isinstance(cells_value, list):
            raise ValueError("annotation.table.cells must be an array")
        if len(cells_value) > 1_500:
            raise ValueError("annotation table exceeds maximum 1500 cells")
        if len(cells_value) != cell_count:
            raise ValueError("cells must form a complete rectangular matrix")
        cells: list[CellReference] = []
        coordinates: set[tuple[int, int]] = set()
        for index, cell_value in enumerate(cells_value):
            cell = _closed_mapping(
                cell_value,
                fields=_CELL_FIELDS,
                name=f"annotation.table.cells[{index}]",
            )
            row = _nonnegative_int(
                cell["row"], f"annotation.table.cells[{index}].row"
            )
            column = _nonnegative_int(
                cell["column"], f"annotation.table.cells[{index}].column"
            )
            bbox = _bbox(
                cell["bbox"], f"annotation.table.cells[{index}].bbox"
            )
            text = _string(
                cell["text"],
                f"annotation.table.cells[{index}].text",
                nonempty=False,
            )
            blank = _bool(
                cell["blank"], f"annotation.table.cells[{index}].blank"
            )
            if blank and text != "":
                raise ValueError("blank cell must have empty text")
            if not blank and not text.strip():
                raise ValueError("nonblank cell must contain text")
            x1, y1, x2, y2 = bbox
            tx1, ty1, tx2, ty2 = table_bbox
            if x1 < tx1 or y1 < ty1 or x2 > tx2 or y2 > ty2:
                raise ValueError("cell box lies outside table bounds")
            if row >= rows or column >= columns:
                raise ValueError("cells must form a complete rectangular matrix")
            if (row, column) in coordinates:
                raise ValueError("cells must form a complete rectangular matrix")
            coordinates.add((row, column))
            cells.append(CellReference(row, column, bbox, text, blank))
        if len(coordinates) != cell_count:
            raise ValueError("cells must form a complete rectangular matrix")

        by_coordinate = {(cell.row, cell.column): cell for cell in cells}
        ordered = tuple(
            by_coordinate[(row, column)]
            for row in range(rows)
            for column in range(columns)
        )
        for cell in ordered:
            if cell.column:
                left_cell = by_coordinate[(cell.row, cell.column - 1)]
                if _boxes_overlap(left_cell.bbox, cell.bbox):
                    raise ValueError("overlapping cells are not allowed")
            if cell.row:
                upper_cell = by_coordinate[(cell.row - 1, cell.column)]
                if _boxes_overlap(upper_cell.bbox, cell.bbox):
                    raise ValueError("overlapping cells are not allowed")

        row_bounds = [
            (
                by_coordinate[(row, 0)].bbox[1],
                by_coordinate[(row, 0)].bbox[3],
            )
            for row in range(rows)
        ]
        column_bounds = [
            (
                by_coordinate[(0, column)].bbox[0],
                by_coordinate[(0, column)].bbox[2],
            )
            for column in range(columns)
        ]
        if (
            column_bounds[0][0] != table_bbox[0]
            or row_bounds[0][0] != table_bbox[1]
            or column_bounds[-1][1] != table_bbox[2]
            or row_bounds[-1][1] != table_bbox[3]
            or any(
                column_bounds[index][1] != column_bounds[index + 1][0]
                for index in range(columns - 1)
            )
            or any(
                row_bounds[index][1] != row_bounds[index + 1][0]
                for index in range(rows - 1)
            )
            or any(
                cell.bbox
                != (
                    column_bounds[cell.column][0],
                    row_bounds[cell.row][0],
                    column_bounds[cell.column][1],
                    row_bounds[cell.row][1],
                )
                for cell in ordered
            )
        ):
            raise ValueError("cell boxes must share rectangular grid topology")
        table = TableReference(
            bbox=table_bbox,
            rows=rows,
            columns=columns,
            cells=ordered,
        )

    if require_verified_holdout and split == "holdout":
        if review_status != "human_verified":
            raise ValueError("holdout annotations must be human_verified")
    return PageAnnotation(
        page_number=page_number,
        split=split,
        negative=negative,
        source_sha256=source_sha256,
        render_sha256=render_sha256,
        review_status=review_status,
        reviewer=reviewer,
        revision=revision,
        table=table,
    )


def _boxes_overlap(
    left: tuple[int, int, int, int],
    right: tuple[int, int, int, int],
) -> bool:
    return (
        min(left[2], right[2]) > max(left[0], right[0])
        and min(left[3], right[3]) > max(left[1], right[1])
    )


def template_fingerprint(annotation: PageAnnotation) -> str | None:
    """Hash scale/position-invariant rectangular-grid topology."""
    table = annotation.table
    if table is None:
        return None
    left, top, right, bottom = table.bbox
    width = right - left
    height = bottom - top

    def normalize(value: int, origin: int, extent: int) -> int:
        return round((value - origin) * 1_000 / extent)

    descriptor = {
        "schema_version": 1,
        "rows": table.rows,
        "columns": table.columns,
        "cells": [
            [
                cell.row,
                cell.column,
                normalize(cell.bbox[0], left, width),
                normalize(cell.bbox[1], top, height),
                normalize(cell.bbox[2], left, width),
                normalize(cell.bbox[3], top, height),
            ]
            for cell in table.cells
        ],
    }
    encoded = json.dumps(
        descriptor,
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def load_annotation(
    path: Path,
    *,
    expected_render_sha256: str,
) -> PageAnnotation:
    """Load one closed annotation and enforce the holdout review gate."""
    try:
        payload = json.loads(path.read_bytes())
    except (json.JSONDecodeError, UnicodeDecodeError) as error:
        raise ValueError("annotation is not valid UTF-8 JSON") from error
    return _parse_annotation(
        payload,
        expected_render_sha256=expected_render_sha256,
        require_verified_holdout=True,
    )


def _relative_private_path(value: object, name: str) -> str:
    text = _string(value, name)
    pure = PurePosixPath(text)
    if pure.is_absolute() or ".." in pure.parts or pure.as_posix() != text:
        raise ValueError(f"{name} must be a normalized private relative path")
    return text


def _resolve_private_path(root: Path, relative: str) -> Path:
    candidate = (root / relative).resolve()
    try:
        candidate.relative_to(root.resolve())
    except ValueError as error:
        raise PermissionError("private artifact path escapes manifest root") from error
    return candidate


def _parse_manifest_page(value: object, index: int) -> ManifestPage:
    page = _closed_mapping(
        value, fields=_MANIFEST_PAGE_FIELDS, name=f"manifest.pages[{index}]"
    )
    page_number = _positive_int(
        page["page_number"], f"manifest.pages[{index}].page_number"
    )
    split = _string(page["split"], f"manifest.pages[{index}].split")
    if split not in {"tuning", "holdout"}:
        raise ValueError(f"manifest.pages[{index}].split is invalid")
    negative = _bool(page["negative"], f"manifest.pages[{index}].negative")
    if negative and split != "holdout":
        raise ValueError("negative pages must belong to the holdout split")
    template_family = _string(
        page["template_family"], f"manifest.pages[{index}].template_family"
    )
    fingerprint_value = page["template_fingerprint"]
    if negative:
        if fingerprint_value is not None:
            raise ValueError("negative page template_fingerprint must be null")
        fingerprint = None
    else:
        fingerprint = _sha256(
            fingerprint_value,
            f"manifest.pages[{index}].template_fingerprint",
        )
    render_path = _relative_private_path(
        page["render_path"], f"manifest.pages[{index}].render_path"
    )
    render_sha256 = _sha256(
        page["render_sha256"], f"manifest.pages[{index}].render_sha256"
    )
    annotation_path = _relative_private_path(
        page["annotation_path"], f"manifest.pages[{index}].annotation_path"
    )
    annotation_sha256 = _sha256(
        page["annotation_sha256"],
        f"manifest.pages[{index}].annotation_sha256",
    )
    review_status = _string(
        page["review_status"], f"manifest.pages[{index}].review_status"
    )
    if review_status not in {"draft", "human_verified"}:
        raise ValueError(f"manifest.pages[{index}].review_status is invalid")
    reviewer = _string(page["reviewer"], f"manifest.pages[{index}].reviewer")
    revision = _nonnegative_int(
        page["revision"], f"manifest.pages[{index}].revision"
    )
    if review_status == "human_verified" and revision < 1:
        raise ValueError("human-verified manifest revisions must be positive")
    return ManifestPage(
        page_number=page_number,
        split=split,
        negative=negative,
        template_family=template_family,
        template_fingerprint=fingerprint,
        render_path=render_path,
        render_sha256=render_sha256,
        annotation_path=annotation_path,
        annotation_sha256=annotation_sha256,
        review_status=review_status,
        reviewer=reviewer,
        revision=revision,
    )


def _validate_manifest_pages(pages: Sequence[ManifestPage]) -> None:
    page_numbers = [page.page_number for page in pages]
    if len(set(page_numbers)) != len(page_numbers):
        raise ValueError("manifest contains a duplicate page number")
    tuning = [page for page in pages if page.split == "tuning" and not page.negative]
    holdout = [
        page for page in pages if page.split == "holdout" and not page.negative
    ]
    negative = [page for page in pages if page.negative]
    if (len(tuning), len(holdout), len(negative)) != (6, 3, 3):
        raise ValueError("manifest must contain 6 tuning, 3 holdout, and 3 negative pages")
    page_450 = [page for page in pages if page.page_number == 450]
    if (
        len(page_450) != 1
        or page_450[0].split != "tuning"
        or page_450[0].negative
    ):
        raise ValueError("page 450 must be present in tuning")

    table_pages = tuning + holdout
    fingerprints = [page.template_fingerprint for page in table_pages]
    if len(set(fingerprints)) != len(fingerprints):
        raise ValueError("duplicate structural template fingerprint")
    families = [page.template_family for page in table_pages]
    if len(set(families)) != len(families):
        tuning_families = {page.template_family for page in tuning}
        holdout_families = {page.template_family for page in holdout}
        if tuning_families & holdout_families:
            raise ValueError("table template leakage between tuning and holdout")
        raise ValueError("table pages must use distinct template families")
    for index, left in enumerate(table_pages):
        for right in table_pages[index + 1 :]:
            if abs(left.page_number - right.page_number) <= 1:
                raise ValueError("adjacent-template leakage is not allowed")


def load_manifest(
    path: Path,
    *,
    mode: Literal["tuning", "holdout"],
) -> CorpusManifest:
    """Load a frozen manifest with split-aware artifact access."""
    if mode not in {"tuning", "holdout"}:
        raise ValueError("manifest mode must be tuning or holdout")
    raw = path.read_bytes()
    try:
        payload = json.loads(raw)
    except (json.JSONDecodeError, UnicodeDecodeError) as error:
        raise ValueError("manifest is not valid UTF-8 JSON") from error
    manifest = _closed_mapping(payload, fields=_MANIFEST_FIELDS, name="manifest")
    if manifest["schema_version"] != 1 or not _is_int(manifest["schema_version"]):
        raise ValueError("manifest.schema_version must equal 1")
    source = _closed_mapping(
        manifest["source"],
        fields=_MANIFEST_SOURCE_FIELDS,
        name="manifest.source",
    )
    if source["id"] != CANONICAL_SOURCE_ID:
        raise ValueError("manifest source id is not canonical")
    source_sha256 = _sha256(source["sha256"], "manifest.source.sha256")
    if source_sha256 != CANONICAL_SOURCE_SHA256:
        raise ValueError("manifest source SHA-256 is not canonical")
    source_size = _positive_int(
        source["size_bytes"], "manifest.source.size_bytes"
    )
    if source_size != CANONICAL_SOURCE_SIZE:
        raise ValueError("manifest source size is not canonical")
    pages_value = manifest["pages"]
    if not isinstance(pages_value, list):
        raise ValueError("manifest.pages must be an array")
    pages = tuple(
        _parse_manifest_page(page, index) for index, page in enumerate(pages_value)
    )
    _validate_manifest_pages(pages)
    tuning = tuple(
        page for page in pages if page.split == "tuning" and not page.negative
    )
    holdout = tuple(
        page for page in pages if page.split == "holdout" and not page.negative
    )
    negative = tuple(page for page in pages if page.negative)
    return CorpusManifest(
        source_id=source["id"],
        source_sha256=source_sha256,
        source_size_bytes=source_size,
        tuning=tuning,
        holdout=holdout,
        negative=negative,
        manifest_sha256=hashlib.sha256(raw).hexdigest(),
        _root=path.resolve().parent,
        _mode=mode,
    )


def _atomic_write_bytes(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.",
        suffix=".tmp",
        dir=path.parent,
    )
    try:
        with os.fdopen(descriptor, "wb") as temporary:
            temporary.write(data)
            temporary.flush()
            os.fsync(temporary.fileno())
        os.replace(temporary_name, path)
    except BaseException:
        try:
            os.unlink(temporary_name)
        except FileNotFoundError:
            pass
        raise


def _selection(value: object, index: int) -> dict[str, Any]:
    selection = _closed_mapping(
        value, fields=_SELECTION_FIELDS, name=f"pages[{index}]"
    )
    page_number = _positive_int(selection["page_number"], f"pages[{index}].page_number")
    split = _string(selection["split"], f"pages[{index}].split")
    if split not in {"tuning", "holdout"}:
        raise ValueError(f"pages[{index}].split is invalid")
    negative = _bool(selection["negative"], f"pages[{index}].negative")
    if negative and split != "holdout":
        raise ValueError("negative pages must belong to holdout")
    return {
        "page_number": page_number,
        "split": split,
        "negative": negative,
        "template_family": _string(
            selection["template_family"], f"pages[{index}].template_family"
        ),
    }


def render_frozen_pages(
    source: Path,
    *,
    pages: Sequence[Mapping[str, Any]],
    output_root: Path,
    manifest_path: Path,
    config_path: Path = DEFAULT_CONFIG,
) -> tuple[RenderedPage, ...]:
    """Render selected pages atomically and bind renders plus annotations."""
    config = load_config(config_path)
    source_size = source.stat().st_size
    max_bytes = config["source"]["max_bytes"]
    if source_size > max_bytes:
        raise ValueError("source size limit exceeded")
    source_bytes = source.read_bytes()
    source_sha256 = hashlib.sha256(source_bytes).hexdigest()
    if source_sha256 != config["source"]["expected_sha256"]:
        raise ValueError("source SHA-256 mismatch")
    if manifest_path.resolve().parent != output_root.resolve():
        raise ValueError("manifest must be written directly under output_root")

    selections = tuple(_selection(value, index) for index, value in enumerate(pages))
    render_config = config["render"]
    limits = RenderLimits(
        dpi=render_config["dpi"],
        max_pixels=render_config["max_pixels"],
        max_dimension=render_config["max_dimension"],
    )
    render_directory = output_root / "renders"
    rendered_pages: list[RenderedPage] = []
    with open_pdf(source_bytes) as document:
        for selection in selections:
            page_number = selection["page_number"]
            if page_number > len(document):
                raise ValueError(f"page {page_number} is outside the source PDF")
            source_page = document[page_number - 1]
            try:
                image = render_page(source_page, limits)
                try:
                    width, height = image.size
                    output = render_directory / f"page-{page_number:04d}.png"
                    output.parent.mkdir(parents=True, exist_ok=True)
                    descriptor, temporary_name = tempfile.mkstemp(
                        prefix=f".{output.name}.",
                        suffix=".tmp",
                        dir=output.parent,
                    )
                    os.close(descriptor)
                    try:
                        image.save(temporary_name, format="PNG")
                        with Path(temporary_name).open("rb") as temporary:
                            os.fsync(temporary.fileno())
                        os.replace(temporary_name, output)
                    except BaseException:
                        Path(temporary_name).unlink(missing_ok=True)
                        raise
                finally:
                    image.close()
            finally:
                source_page.close()
            render_sha256 = hashlib.sha256(output.read_bytes()).hexdigest()
            rendered_pages.append(
                RenderedPage(
                    page_number=page_number,
                    split=selection["split"],
                    negative=selection["negative"],
                    template_family=selection["template_family"],
                    path=output,
                    sha256=render_sha256,
                    width=width,
                    height=height,
                )
            )

    manifest_pages: list[dict[str, Any]] = []
    for rendered in rendered_pages:
        annotation_path = (
            output_root / "annotations" / f"page-{rendered.page_number:04d}.json"
        )
        annotation_bytes = annotation_path.read_bytes()
        try:
            annotation_payload = json.loads(annotation_bytes)
        except (json.JSONDecodeError, UnicodeDecodeError) as error:
            raise ValueError("annotation is not valid UTF-8 JSON") from error
        annotation = _parse_annotation(
            annotation_payload,
            expected_render_sha256=rendered.sha256,
            require_verified_holdout=False,
        )
        if (
            annotation.source_sha256 != source_sha256
            or annotation.page_number != rendered.page_number
            or annotation.split != rendered.split
            or annotation.negative != rendered.negative
        ):
            raise ValueError("annotation metadata does not match rendered page")
        manifest_pages.append(
            {
                "page_number": rendered.page_number,
                "split": rendered.split,
                "negative": rendered.negative,
                "template_family": rendered.template_family,
                "template_fingerprint": template_fingerprint(annotation),
                "render_path": rendered.path.relative_to(output_root).as_posix(),
                "render_sha256": rendered.sha256,
                "annotation_path": annotation_path.relative_to(
                    output_root
                ).as_posix(),
                "annotation_sha256": hashlib.sha256(annotation_bytes).hexdigest(),
                "review_status": annotation.review_status,
                "reviewer": annotation.reviewer,
                "revision": annotation.revision,
            }
        )
    _validate_manifest_pages(
        tuple(
            _parse_manifest_page(page, index)
            for index, page in enumerate(manifest_pages)
        )
    )
    payload = {
        "schema_version": 1,
        "source": {
            "id": CANONICAL_SOURCE_ID,
            "sha256": source_sha256,
            "size_bytes": source_size,
        },
        "pages": manifest_pages,
    }
    _atomic_write_bytes(
        manifest_path,
        (json.dumps(payload, ensure_ascii=False, indent=2) + "\n").encode("utf-8"),
    )
    return tuple(rendered_pages)


def write_corpus_report(manifest: CorpusManifest, output: Path) -> None:
    """Write the privacy-preserving corpus inventory and no annotation data."""
    pages = manifest.tuning + manifest.holdout + manifest.negative
    verified = sum(page.review_status == "human_verified" for page in pages)
    template_families = len(
        {
            page.template_fingerprint
            for page in manifest.tuning + manifest.holdout
        }
    ) + (1 if manifest.negative else 0)
    report = (
        "# Ruled-table OCR corpus\n"
        "\n"
        f"- Source id: `{manifest.source_id}`\n"
        f"- Source SHA-256: `{manifest.source_sha256}`\n"
        f"- Manifest SHA-256: `{manifest.manifest_sha256}`\n"
        f"- Tuning table pages: {len(manifest.tuning)}\n"
        f"- Holdout table pages: {len(manifest.holdout)}\n"
        f"- Negative pages: {len(manifest.negative)}\n"
        f"- Human-verified annotations: {verified}/{len(pages)}\n"
        f"- Distinct template families: {template_families}\n"
        "- Ground-truth text tracked: no\n"
    )
    _atomic_write_bytes(output, report.encode("utf-8"))


@dataclass(frozen=True, slots=True)
class CellMatchCounts:
    tp: int
    fp: int
    fn: int

    @property
    def precision(self) -> float:
        denominator = self.tp + self.fp
        return self.tp / denominator if denominator else 1.0

    @property
    def recall(self) -> float:
        denominator = self.tp + self.fn
        return self.tp / denominator if denominator else 1.0

    @property
    def f1(self) -> float:
        denominator = self.precision + self.recall
        return (
            2 * self.precision * self.recall / denominator
            if denominator
            else 0.0
        )


@dataclass(frozen=True, slots=True)
class PositionedBox:
    row: int
    column: int
    box: Box

    def __post_init__(self) -> None:
        if (
            not _is_int(self.row)
            or not _is_int(self.column)
            or self.row < 0
            or self.column < 0
        ):
            raise ValueError("positioned box coordinates must be nonnegative integers")
        if not isinstance(self.box, Box):
            raise TypeError("positioned box must contain a Box")


@dataclass(frozen=True, slots=True)
class CellContentCounts:
    character_edits: int
    reference_characters: int
    word_edits: int
    reference_words: int
    blank_correct: int
    blank_total: int
    cells: Mapping[
        tuple[int, int],
        tuple[int, int, int, int, bool, bool],
    ]


@dataclass(frozen=True, slots=True)
class ArtifactBindings:
    config_sha256: str
    manifest_sha256: str
    source_sha256: str
    render_sha256_by_page: Mapping[int, str]
    tesseract_sha256: str
    tessdata_sha256: str
    host_sha256: str
    toolchain_sha256: str


_ARTIFACT_FIELDS = frozenset(
    {
        "schema_version",
        "split",
        "source",
        "config_sha256",
        "manifest_sha256",
        "host",
        "toolchain",
        "access",
        "candidates",
        "records",
        "aggregates",
        "winner_id",
        "decision",
    }
)
_ARTIFACT_SOURCE_FIELDS = frozenset({"id", "sha256", "size_bytes"})
_HOST_FIELDS = frozenset(
    {"platform", "architecture", "logical_cpus", "memory_bytes"}
)
_TOOLCHAIN_FIELDS = frozenset({"python", "pillow", "tesseract"})
_ACCESS_FIELDS = frozenset(
    {
        "tuning_pages_opened",
        "holdout_pages_opened",
        "negative_pages_opened",
    }
)
_ARTIFACT_CANDIDATE_FIELDS = frozenset({"id", "configuration_sha256"})
_RECORD_FIELDS = frozenset(
    {
        "id",
        "candidate_id",
        "page_number",
        "split",
        "negative",
        "status",
        "rows",
        "columns",
        "reference_rows",
        "reference_columns",
        "predicted_boxes",
        "reference_boxes",
        "cells",
        "elapsed_seconds",
        "peak_rss_bytes",
        "resource",
        "bindings",
    }
)
_ARTIFACT_BOX_FIELDS = frozenset({"row", "column", "bbox"})
_ARTIFACT_CELL_FIELDS = frozenset(
    {
        "row",
        "column",
        "character_edits",
        "reference_characters",
        "word_edits",
        "reference_words",
        "reference_blank",
        "predicted_blank",
        "prediction_present",
    }
)
_RESOURCE_FIELDS = frozenset(
    {"timed_out", "resource_violation", "cleanup_failed"}
)
_BINDING_FIELDS = frozenset(
    {
        "config_sha256",
        "manifest_sha256",
        "source_sha256",
        "render_sha256",
        "tesseract_sha256",
        "tessdata_sha256",
        "host_sha256",
        "toolchain_sha256",
    }
)
_FROZEN_WINNER_FIELDS = frozenset(
    {
        "schema_version",
        "winner_id",
        "configuration_sha256",
        "config_sha256",
        "manifest_sha256",
        "tuning_artifact_sha256",
    }
)
_AGGREGATE_FIELDS = frozenset(
    {
        "candidate_id",
        "record_count",
        "table_pages",
        "negative_pages",
        "failures",
        "timeouts",
        "resource_violations",
        "cleanup_failures",
        "false_positives",
        "wrong_grids",
        "exact_grid_pages",
        "cell_tp",
        "cell_fp",
        "cell_fn",
        "character_edits",
        "reference_characters",
        "word_edits",
        "reference_words",
        "blank_correct",
        "blank_total",
        "cell_precision",
        "cell_recall",
        "cell_f1",
        "cell_cer",
        "cell_wer",
        "empty_cell_accuracy",
        "median_page_latency_seconds",
        "maximum_page_latency_seconds",
        "peak_rss_bytes",
    }
)
_INTEGER_AGGREGATE_FIELDS = _AGGREGATE_FIELDS - {
    "candidate_id",
    "cell_precision",
    "cell_recall",
    "cell_f1",
    "cell_cer",
    "cell_wer",
    "empty_cell_accuracy",
    "median_page_latency_seconds",
    "maximum_page_latency_seconds",
}
_RECORD_STATUSES = frozenset(
    {
        "detected",
        "not_detected",
        "unsupported",
        "invalid_grid",
        "timeout",
        "output_limit",
        "candidate_error",
        "resource_limit",
        "cleanup_error",
    }
)
_FORBIDDEN_RAW_KEYS = frozenset(
    {
        "text",
        "recognized_text",
        "reference",
        "markdown",
        "path",
        "render_path",
        "annotation_path",
        "environment",
        "env",
        "error",
        "error_detail",
        "image",
        "image_bytes",
    }
)
_TUNING_PAGES = frozenset({255, 460, 537, 450, 541, 772})
_HOLDOUT_PAGES = frozenset({394, 503, 809})
_NEGATIVE_PAGES = frozenset({20, 60, 100})


def _box_iou(left: Box, right: Box) -> float:
    intersection_width = max(
        0, min(left.right, right.right) - max(left.left, right.left)
    )
    intersection_height = max(
        0, min(left.bottom, right.bottom) - max(left.top, right.top)
    )
    intersection = intersection_width * intersection_height
    union = left.area + right.area - intersection
    return intersection / union if union else 0.0


def match_cells(
    references: Sequence[Box | PositionedBox],
    predictions: Sequence[Box | PositionedBox],
    *,
    threshold: float,
) -> CellMatchCounts:
    """Greedily match boxes one-to-one with deterministic IoU ordering."""
    if (
        isinstance(threshold, bool)
        or not isinstance(threshold, (int, float))
        or not math.isfinite(float(threshold))
        or not 0 <= float(threshold) <= 1
    ):
        raise ValueError("threshold must be a finite fraction")
    def positioned(
        values: Sequence[Box | PositionedBox],
    ) -> tuple[PositionedBox, ...]:
        output: list[PositionedBox] = []
        for index, value in enumerate(values):
            if isinstance(value, PositionedBox):
                output.append(value)
            elif isinstance(value, Box):
                output.append(PositionedBox(index, 0, value))
            else:
                raise TypeError("cell boxes must be Box or PositionedBox values")
        return tuple(output)

    positioned_references = positioned(references)
    positioned_predictions = positioned(predictions)
    candidates = [
        (
            iou,
            reference.row,
            reference.column,
            prediction.row,
            prediction.column,
            reference_index,
            prediction_index,
        )
        for reference_index, reference in enumerate(positioned_references)
        for prediction_index, prediction in enumerate(positioned_predictions)
        if (iou := _box_iou(reference.box, prediction.box)) >= threshold
    ]
    candidates.sort(key=lambda item: (-item[0], *item[1:5]))
    used_references: set[int] = set()
    used_predictions: set[int] = set()
    for (
        _iou,
        _reference_row,
        _reference_column,
        _prediction_row,
        _prediction_column,
        reference_index,
        prediction_index,
    ) in candidates:
        if (
            reference_index in used_references
            or prediction_index in used_predictions
        ):
            continue
        used_references.add(reference_index)
        used_predictions.add(prediction_index)
    tp = len(used_references)
    return CellMatchCounts(
        tp=tp,
        fp=len(predictions) - tp,
        fn=len(references) - tp,
    )


def measure_cell_content(
    references: Mapping[tuple[int, int], tuple[str, bool]],
    predictions: Mapping[tuple[int, int], str],
    *,
    reference_shape: tuple[int, int] | None = None,
    predicted_shape: tuple[int, int] | None = None,
) -> CellContentCounts:
    """Return additive content and blank counts without retaining cell text."""
    if (
        reference_shape is not None
        and predicted_shape is not None
        and reference_shape != predicted_shape
    ):
        raise ValueError(
            "content metrics require exact rows and columns to be confirmed"
        )
    cells: dict[
        tuple[int, int],
        tuple[int, int, int, int, bool, bool],
    ] = {}
    character_edits = reference_characters = 0
    word_edits = reference_words = 0
    blank_correct = 0
    for coordinate in sorted(set(references) | set(predictions)):
        reference_value = references.get(coordinate)
        prediction_present = coordinate in predictions
        if reference_value is None:
            reference_text = ""
            reference_blank = False
        else:
            reference_text, reference_blank = reference_value
            if not isinstance(reference_text, str) or not isinstance(
                reference_blank, bool
            ):
                raise TypeError("reference cells must contain text and blank flags")
        hypothesis = predictions.get(coordinate, "")
        if not isinstance(hypothesis, str):
            raise TypeError("predicted cells must contain strings")
        counts = error_counts(reference_text, hypothesis)
        predicted_blank = not bool(normalize_for_metric(hypothesis))
        cells[coordinate] = (
            counts.character_edits,
            counts.reference_characters,
            counts.word_edits,
            counts.reference_words,
            reference_blank,
            predicted_blank if prediction_present else False,
        )
        character_edits += counts.character_edits
        reference_characters += counts.reference_characters
        word_edits += counts.word_edits
        reference_words += counts.reference_words
        if (
            reference_value is not None
            and prediction_present
            and reference_blank == predicted_blank
        ):
            blank_correct += 1
    return CellContentCounts(
        character_edits=character_edits,
        reference_characters=reference_characters,
        word_edits=word_edits,
        reference_words=reference_words,
        blank_correct=blank_correct,
        blank_total=len(references),
        cells=cells,
    )


def _canonical_json_sha256(value: Any) -> str:
    encoded = json.dumps(
        value, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _command_version(command: Sequence[str]) -> str:
    result = subprocess.run(
        list(command),
        capture_output=True,
        text=True,
        check=True,
        timeout=30,
    )
    output = result.stdout or result.stderr
    return output.splitlines()[0].strip()


def _host_description() -> dict[str, Any]:
    return {
        "platform": platform.platform(),
        "architecture": platform.machine(),
        "logical_cpus": psutil.cpu_count(logical=True) or 0,
        "memory_bytes": psutil.virtual_memory().total,
    }


def _toolchain_description() -> dict[str, str]:
    return {
        "python": platform.python_version(),
        "pillow": importlib.metadata.version("Pillow"),
        "tesseract": _command_version(["tesseract", "--version"]),
    }


def _canonical_artifact_bindings() -> ArtifactBindings:
    config_raw = DEFAULT_CONFIG.read_bytes()
    manifest_raw = DEFAULT_MANIFEST.read_bytes()
    if hashlib.sha256(config_raw).hexdigest() != CANONICAL_CONFIG_SHA256:
        raise ValueError("canonical config SHA-256 mismatch")
    if hashlib.sha256(manifest_raw).hexdigest() != CANONICAL_MANIFEST_SHA256:
        raise ValueError("canonical manifest SHA-256 mismatch")
    config = load_config(DEFAULT_CONFIG)
    _verify_tessdata(DEFAULT_TESSDATA, config)
    manifest_payload = json.loads(manifest_raw)
    pages = manifest_payload.get("pages")
    if not isinstance(pages, list):
        raise ValueError("canonical manifest pages are invalid")
    render_hashes: dict[int, str] = {}
    for page in pages:
        if not isinstance(page, dict):
            raise ValueError("canonical manifest page is invalid")
        page_number = _positive_int(
            page.get("page_number"), "canonical manifest page_number"
        )
        render_hashes[page_number] = _sha256(
            page.get("render_sha256"), "canonical manifest render_sha256"
        )
    tesseract = shutil.which("tesseract")
    if tesseract is None:
        raise ValueError("canonical Tesseract executable is unavailable")
    host = _host_description()
    toolchain = _toolchain_description()
    return ArtifactBindings(
        config_sha256=hashlib.sha256(config_raw).hexdigest(),
        manifest_sha256=hashlib.sha256(manifest_raw).hexdigest(),
        source_sha256=CANONICAL_SOURCE_SHA256,
        render_sha256_by_page=render_hashes,
        tesseract_sha256=_sha256_file(Path(tesseract)),
        tessdata_sha256=_sha256_file(DEFAULT_TESSDATA / "vie.traineddata"),
        host_sha256=_canonical_json_sha256(host),
        toolchain_sha256=_canonical_json_sha256(toolchain),
    )


def _artifact_candidate_descriptors(config: Mapping[str, Any]) -> list[dict[str, str]]:
    candidates = config["detector_candidates"]
    return [
        {
            "id": candidate["id"],
            "configuration_sha256": _canonical_json_sha256(candidate),
        }
        for candidate in candidates
    ]


def _rate(edits: int, reference_units: int) -> float:
    if reference_units:
        return edits / reference_units
    return float(edits > 0)


def _artifact_box(value: Mapping[str, Any]) -> PositionedBox:
    bbox = value["bbox"]
    return PositionedBox(
        value["row"],
        value["column"],
        Box(bbox[0], bbox[1], bbox[2], bbox[3]),
    )


def _aggregate_candidate(
    candidate_id: str,
    records: Sequence[Mapping[str, Any]],
    *,
    threshold: float = 0.80,
) -> dict[str, Any]:
    table_records = [record for record in records if not record["negative"]]
    negative_records = [record for record in records if record["negative"]]
    match_totals = CellMatchCounts(0, 0, 0)
    for record in table_records:
        counts = match_cells(
            [_artifact_box(box) for box in record["reference_boxes"]],
            [_artifact_box(box) for box in record["predicted_boxes"]],
            threshold=threshold,
        )
        match_totals = CellMatchCounts(
            match_totals.tp + counts.tp,
            match_totals.fp + counts.fp,
            match_totals.fn + counts.fn,
        )
    cells = [cell for record in records for cell in record["cells"]]
    character_edits = sum(cell["character_edits"] for cell in cells)
    reference_characters = sum(cell["reference_characters"] for cell in cells)
    word_edits = sum(cell["word_edits"] for cell in cells)
    reference_words = sum(cell["reference_words"] for cell in cells)
    blank_correct = sum(
        cell["prediction_present"]
        and cell["reference_blank"] == cell["predicted_blank"]
        for cell in cells
        if (cell["row"], cell["column"])
        in {
            (box["row"], box["column"])
            for record in records
            for box in record["reference_boxes"]
        }
    )
    blank_total = sum(len(record["reference_boxes"]) for record in table_records)
    exact_grid_pages = sum(
        record["status"] == "detected"
        and record["rows"] == record["reference_rows"]
        and record["columns"] == record["reference_columns"]
        for record in table_records
    )
    elapsed = sorted(float(record["elapsed_seconds"]) for record in records)
    successful_statuses = {"detected", "not_detected"}
    return {
        "candidate_id": candidate_id,
        "record_count": len(records),
        "table_pages": len(table_records),
        "negative_pages": len(negative_records),
        "failures": sum(
            record["status"] not in successful_statuses for record in records
        ),
        "timeouts": sum(record["resource"]["timed_out"] for record in records),
        "resource_violations": sum(
            record["resource"]["resource_violation"] for record in records
        ),
        "cleanup_failures": sum(
            record["resource"]["cleanup_failed"] for record in records
        ),
        "false_positives": sum(
            record["status"] == "detected" for record in negative_records
        ),
        "wrong_grids": len(table_records) - exact_grid_pages,
        "exact_grid_pages": exact_grid_pages,
        "cell_tp": match_totals.tp,
        "cell_fp": match_totals.fp,
        "cell_fn": match_totals.fn,
        "character_edits": character_edits,
        "reference_characters": reference_characters,
        "word_edits": word_edits,
        "reference_words": reference_words,
        "blank_correct": blank_correct,
        "blank_total": blank_total,
        "cell_precision": match_totals.precision,
        "cell_recall": match_totals.recall,
        "cell_f1": match_totals.f1,
        "cell_cer": _rate(character_edits, reference_characters),
        "cell_wer": _rate(word_edits, reference_words),
        "empty_cell_accuracy": (
            blank_correct / blank_total if blank_total else 1.0
        ),
        "median_page_latency_seconds": statistics.median(elapsed),
        "maximum_page_latency_seconds": max(elapsed),
        "peak_rss_bytes": max(record["peak_rss_bytes"] for record in records),
    }


def _recompute_aggregates(payload: Mapping[str, Any]) -> list[dict[str, Any]]:
    candidate_ids = [candidate["id"] for candidate in payload["candidates"]]
    if payload["split"] == "holdout":
        candidate_ids = [payload["winner_id"]] if payload["winner_id"] else []
    return [
        _aggregate_candidate(
            candidate_id,
            [
                record
                for record in payload["records"]
                if record["candidate_id"] == candidate_id
            ],
        )
        for candidate_id in candidate_ids
    ]


def _records_have_expected_cardinality(payload: Mapping[str, Any]) -> bool:
    split = payload.get("split")
    records = payload.get("records")
    candidates = payload.get("candidates")
    if not isinstance(records, list) or not isinstance(candidates, list):
        return False
    if split == "tuning":
        expected_pages = _TUNING_PAGES
        expected_candidates = {
            candidate.get("id")
            for candidate in candidates
            if isinstance(candidate, Mapping)
        }
    elif split == "holdout":
        expected_pages = _HOLDOUT_PAGES | _NEGATIVE_PAGES
        winner = payload.get("winner_id")
        expected_candidates = {winner} if isinstance(winner, str) else set()
    else:
        return False
    actual = [
        (record.get("candidate_id"), record.get("page_number"))
        for record in records
        if isinstance(record, Mapping)
    ]
    expected = {
        (candidate_id, page)
        for candidate_id in expected_candidates
        for page in expected_pages
    }
    return len(actual) == len(set(actual)) and set(actual) == expected


def derive_tuning_winner(payload: Mapping[str, Any]) -> str | None:
    """Select the deterministic winner from complete tuning aggregates."""
    if payload.get("split") != "tuning" or not _records_have_expected_cardinality(
        payload
    ):
        return None
    aggregates = payload.get("aggregates")
    if not isinstance(aggregates, list):
        return None
    eligible = [
        aggregate
        for aggregate in aggregates
        if isinstance(aggregate, Mapping)
        and aggregate.get("record_count") == 6
        and aggregate.get("table_pages") == 6
        and aggregate.get("negative_pages") == 0
        and all(
            aggregate.get(field) == 0
            for field in (
                "failures",
                "timeouts",
                "resource_violations",
                "cleanup_failures",
                "false_positives",
                "wrong_grids",
            )
        )
    ]
    if not eligible:
        return None
    ranked = sorted(
        eligible,
        key=lambda aggregate: (
            -aggregate["exact_grid_pages"],
            -aggregate["cell_f1"],
            aggregate["cell_cer"],
            -aggregate["empty_cell_accuracy"],
            aggregate["median_page_latency_seconds"],
            aggregate["candidate_id"],
        ),
    )
    return str(ranked[0]["candidate_id"])


def derive_holdout_decision(
    payload: Mapping[str, Any],
) -> Literal["PASS", "STOP"]:
    """Apply every holdout bound conservatively, never trusting favorable drift."""
    if payload.get("split") != "holdout" or not _records_have_expected_cardinality(
        payload
    ):
        return "STOP"
    aggregates = payload.get("aggregates")
    if not isinstance(aggregates, list) or len(aggregates) != 1:
        return "STOP"
    supplied = aggregates[0]
    if not isinstance(supplied, Mapping):
        return "STOP"
    try:
        actual = _recompute_aggregates(payload)[0]
        conditions = (
            min(supplied["exact_grid_pages"], actual["exact_grid_pages"]) == 3,
            min(supplied["cell_f1"], actual["cell_f1"]) >= 0.95,
            max(supplied["cell_cer"], actual["cell_cer"]) <= 0.05,
            min(
                supplied["empty_cell_accuracy"],
                actual["empty_cell_accuracy"],
            )
            >= 0.98,
            max(supplied["false_positives"], actual["false_positives"]) == 0,
            max(supplied["failures"], actual["failures"]) == 0,
            max(supplied["timeouts"], actual["timeouts"]) == 0,
            max(
                supplied["resource_violations"],
                actual["resource_violations"],
            )
            == 0,
            max(supplied["cleanup_failures"], actual["cleanup_failures"]) == 0,
            max(supplied["peak_rss_bytes"], actual["peak_rss_bytes"])
            < 805_306_368,
            max(
                supplied["maximum_page_latency_seconds"],
                actual["maximum_page_latency_seconds"],
            )
            <= 20.0,
            supplied.get("candidate_id") == payload.get("winner_id"),
        )
    except (KeyError, TypeError, ValueError, IndexError):
        return "STOP"
    return "PASS" if all(conditions) else "STOP"


def _reject_forbidden_raw_keys(value: Any, *, path: str = "artifact") -> None:
    if isinstance(value, Mapping):
        for key, nested in value.items():
            if not isinstance(key, str):
                raise ValueError(f"{path} keys must be strings")
            if key.lower() in _FORBIDDEN_RAW_KEYS:
                raise ValueError(f"{path}.{key} is forbidden in raw evidence")
            _reject_forbidden_raw_keys(nested, path=f"{path}.{key}")
    elif isinstance(value, list):
        for index, nested in enumerate(value):
            _reject_forbidden_raw_keys(nested, path=f"{path}[{index}]")


def _finite_nonnegative_number(value: Any, name: str) -> float:
    number = _number(value, name)
    if not math.isfinite(number) or number < 0:
        raise ValueError(f"{name} must be finite and nonnegative")
    return number


def _validate_artifact_box(value: Any, *, name: str) -> tuple[int, int]:
    box = _closed_mapping(value, fields=_ARTIFACT_BOX_FIELDS, name=name)
    coordinate = (
        _nonnegative_int(box["row"], f"{name}.row"),
        _nonnegative_int(box["column"], f"{name}.column"),
    )
    _bbox(box["bbox"], f"{name}.bbox")
    return coordinate


def _validate_record(
    value: Any,
    *,
    index: int,
    split: str,
    candidate_ids: set[str],
    bindings: ArtifactBindings,
) -> None:
    name = f"artifact.records[{index}]"
    record = _closed_mapping(value, fields=_RECORD_FIELDS, name=name)
    candidate_id = _string(record["candidate_id"], f"{name}.candidate_id")
    if candidate_id not in candidate_ids:
        raise ValueError(f"{name}.candidate_id is not canonical")
    page_number = _positive_int(record["page_number"], f"{name}.page_number")
    expected_pages = (
        _TUNING_PAGES if split == "tuning" else _HOLDOUT_PAGES | _NEGATIVE_PAGES
    )
    if page_number not in expected_pages:
        raise ValueError(f"{name}.page_number is outside the frozen split")
    if record["id"] != f"{split}:{candidate_id}:{page_number}":
        raise ValueError(f"{name}.id is not canonical")
    if record["split"] != split:
        raise ValueError(f"{name}.split is inconsistent")
    negative = _bool(record["negative"], f"{name}.negative")
    if negative != (page_number in _NEGATIVE_PAGES):
        raise ValueError(f"{name}.negative is inconsistent")
    status = _string(record["status"], f"{name}.status")
    if status not in _RECORD_STATUSES:
        raise ValueError(f"{name}.status is unsupported")
    rows = _nonnegative_int(record["rows"], f"{name}.rows")
    columns = _nonnegative_int(record["columns"], f"{name}.columns")
    reference_rows = _nonnegative_int(
        record["reference_rows"], f"{name}.reference_rows"
    )
    reference_columns = _nonnegative_int(
        record["reference_columns"], f"{name}.reference_columns"
    )
    if negative and (reference_rows or reference_columns):
        raise ValueError(f"{name} negative reference dimensions must be zero")
    predicted_boxes = record["predicted_boxes"]
    reference_boxes = record["reference_boxes"]
    if not isinstance(predicted_boxes, list) or not isinstance(reference_boxes, list):
        raise ValueError(f"{name} boxes must be arrays")
    predicted_coordinates = [
        _validate_artifact_box(box, name=f"{name}.predicted_boxes[{box_index}]")
        for box_index, box in enumerate(predicted_boxes)
    ]
    reference_coordinates = [
        _validate_artifact_box(box, name=f"{name}.reference_boxes[{box_index}]")
        for box_index, box in enumerate(reference_boxes)
    ]
    if len(set(predicted_coordinates)) != len(predicted_coordinates):
        raise ValueError(f"{name} has duplicate predicted coordinates")
    if len(set(reference_coordinates)) != len(reference_coordinates):
        raise ValueError(f"{name} has duplicate reference coordinates")
    if predicted_coordinates != sorted(predicted_coordinates):
        raise ValueError(f"{name}.predicted_boxes must be row-major")
    if reference_coordinates != sorted(reference_coordinates):
        raise ValueError(f"{name}.reference_boxes must be row-major")
    if status == "detected":
        if rows * columns != len(predicted_boxes):
            raise ValueError(f"{name} detected grid cardinality is inconsistent")
    elif predicted_boxes or rows or columns:
        raise ValueError(f"{name} non-detected record cannot retain a grid")
    if not negative and reference_rows * reference_columns != len(reference_boxes):
        raise ValueError(f"{name} reference grid cardinality is inconsistent")
    cells = record["cells"]
    if not isinstance(cells, list):
        raise ValueError(f"{name}.cells must be an array")
    cell_coordinates: list[tuple[int, int]] = []
    for cell_index, cell_value in enumerate(cells):
        cell_name = f"{name}.cells[{cell_index}]"
        cell = _closed_mapping(
            cell_value, fields=_ARTIFACT_CELL_FIELDS, name=cell_name
        )
        coordinate = (
            _nonnegative_int(cell["row"], f"{cell_name}.row"),
            _nonnegative_int(cell["column"], f"{cell_name}.column"),
        )
        cell_coordinates.append(coordinate)
        for field_name in (
            "character_edits",
            "reference_characters",
            "word_edits",
            "reference_words",
        ):
            _nonnegative_int(cell[field_name], f"{cell_name}.{field_name}")
        _bool(cell["reference_blank"], f"{cell_name}.reference_blank")
        _bool(cell["predicted_blank"], f"{cell_name}.predicted_blank")
        prediction_present = _bool(
            cell["prediction_present"], f"{cell_name}.prediction_present"
        )
        if not prediction_present and (
            cell["character_edits"] != cell["reference_characters"]
            or cell["word_edits"] != cell["reference_words"]
        ):
            raise ValueError(
                f"{cell_name} missing prediction must record full-reference deletions"
            )
    if len(set(cell_coordinates)) != len(cell_coordinates):
        raise ValueError(f"{name} has duplicate cell coordinates")
    if set(reference_coordinates) - set(cell_coordinates):
        raise ValueError(f"{name} is missing annotated cell evidence")
    if status != "detected" and any(
        cell["prediction_present"] for cell in cells
    ):
        raise ValueError(f"{name} failed table record cannot retain predictions")
    _finite_nonnegative_number(record["elapsed_seconds"], f"{name}.elapsed_seconds")
    _nonnegative_int(record["peak_rss_bytes"], f"{name}.peak_rss_bytes")
    resource = _closed_mapping(
        record["resource"], fields=_RESOURCE_FIELDS, name=f"{name}.resource"
    )
    for field_name in _RESOURCE_FIELDS:
        _bool(resource[field_name], f"{name}.resource.{field_name}")
    expected_resource = {
        "timed_out": status == "timeout",
        "resource_violation": status == "resource_limit",
        "cleanup_failed": status == "cleanup_error",
    }
    if dict(resource) != expected_resource:
        raise ValueError(f"{name}.resource flags are inconsistent")
    record_bindings = _closed_mapping(
        record["bindings"], fields=_BINDING_FIELDS, name=f"{name}.bindings"
    )
    expected_bindings = {
        "config_sha256": bindings.config_sha256,
        "manifest_sha256": bindings.manifest_sha256,
        "source_sha256": bindings.source_sha256,
        "render_sha256": bindings.render_sha256_by_page.get(page_number),
        "tesseract_sha256": bindings.tesseract_sha256,
        "tessdata_sha256": bindings.tessdata_sha256,
        "host_sha256": bindings.host_sha256,
        "toolchain_sha256": bindings.toolchain_sha256,
    }
    for field_name, expected in expected_bindings.items():
        actual = _sha256(
            record_bindings[field_name],
            f"{name}.bindings.{field_name}",
        )
        if actual != expected:
            raise ValueError(f"{name}.{field_name} does not bind canonical input")


def _validate_aggregate(value: Any, *, index: int) -> None:
    name = f"artifact.aggregates[{index}]"
    aggregate = _closed_mapping(value, fields=_AGGREGATE_FIELDS, name=name)
    _string(aggregate["candidate_id"], f"{name}.candidate_id")
    for field_name in _INTEGER_AGGREGATE_FIELDS:
        _nonnegative_int(aggregate[field_name], f"{name}.{field_name}")
    for field_name in (
        "cell_precision",
        "cell_recall",
        "cell_f1",
        "empty_cell_accuracy",
    ):
        _fraction(aggregate[field_name], f"{name}.{field_name}")
    for field_name in (
        "cell_cer",
        "cell_wer",
        "median_page_latency_seconds",
        "maximum_page_latency_seconds",
    ):
        _finite_nonnegative_number(aggregate[field_name], f"{name}.{field_name}")


def validate_artifact(payload: Mapping[str, Any], *, split: str) -> None:
    """Validate closed raw evidence against canonical external inputs."""
    if split not in {"tuning", "holdout"}:
        raise ValueError("split must be tuning or holdout")
    _reject_forbidden_raw_keys(payload)
    artifact = _closed_mapping(payload, fields=_ARTIFACT_FIELDS, name="artifact")
    if artifact["schema_version"] != 1 or not _is_int(artifact["schema_version"]):
        raise ValueError("artifact.schema_version must equal 1")
    if artifact["split"] != split:
        raise ValueError("artifact split is inconsistent")
    bindings = _canonical_artifact_bindings()
    source = _closed_mapping(
        artifact["source"], fields=_ARTIFACT_SOURCE_FIELDS, name="artifact.source"
    )
    if (
        source["id"] != CANONICAL_SOURCE_ID
        or _sha256(source["sha256"], "artifact.source.sha256")
        != bindings.source_sha256
        or _positive_int(source["size_bytes"], "artifact.source.size_bytes")
        != CANONICAL_SOURCE_SIZE
    ):
        raise ValueError("artifact source does not bind the canonical source")
    if (
        _sha256(artifact["config_sha256"], "artifact.config_sha256")
        != bindings.config_sha256
    ):
        raise ValueError("artifact config SHA-256 is not canonical")
    if (
        _sha256(artifact["manifest_sha256"], "artifact.manifest_sha256")
        != bindings.manifest_sha256
    ):
        raise ValueError("artifact manifest SHA-256 is not canonical")
    host = _closed_mapping(artifact["host"], fields=_HOST_FIELDS, name="artifact.host")
    _string(host["platform"], "artifact.host.platform")
    _string(host["architecture"], "artifact.host.architecture")
    _nonnegative_int(host["logical_cpus"], "artifact.host.logical_cpus")
    _positive_int(host["memory_bytes"], "artifact.host.memory_bytes")
    if _canonical_json_sha256(host) != bindings.host_sha256:
        raise ValueError("artifact host hash does not bind the canonical host")
    toolchain = _closed_mapping(
        artifact["toolchain"], fields=_TOOLCHAIN_FIELDS, name="artifact.toolchain"
    )
    for field_name in _TOOLCHAIN_FIELDS:
        _string(toolchain[field_name], f"artifact.toolchain.{field_name}")
    if _canonical_json_sha256(toolchain) != bindings.toolchain_sha256:
        raise ValueError(
            "artifact toolchain hash does not bind the canonical toolchain"
        )
    access = _closed_mapping(
        artifact["access"], fields=_ACCESS_FIELDS, name="artifact.access"
    )
    for field_name in _ACCESS_FIELDS:
        _nonnegative_int(access[field_name], f"artifact.access.{field_name}")
    expected_access = (
        {
            "tuning_pages_opened": 6,
            "holdout_pages_opened": 0,
            "negative_pages_opened": 0,
        }
        if split == "tuning"
        else {
            "tuning_pages_opened": 0,
            "holdout_pages_opened": 3,
            "negative_pages_opened": 3,
        }
    )
    if dict(access) != expected_access:
        raise ValueError("artifact access counts violate split isolation")
    candidates = artifact["candidates"]
    if not isinstance(candidates, list):
        raise ValueError("artifact.candidates must be an array")
    config = load_config(DEFAULT_CONFIG)
    expected_candidates = _artifact_candidate_descriptors(config)
    for index, candidate_value in enumerate(candidates):
        candidate = _closed_mapping(
            candidate_value,
            fields=_ARTIFACT_CANDIDATE_FIELDS,
            name=f"artifact.candidates[{index}]",
        )
        _string(candidate["id"], f"artifact.candidates[{index}].id")
        _sha256(
            candidate["configuration_sha256"],
            f"artifact.candidates[{index}].configuration_sha256",
        )
    if candidates != expected_candidates:
        raise ValueError("artifact candidate hashes are not canonical")
    candidate_ids = {candidate["id"] for candidate in candidates}
    winner_id = artifact["winner_id"]
    if winner_id is not None and winner_id not in candidate_ids:
        raise ValueError("artifact winner_id is not a canonical candidate")
    records = artifact["records"]
    if not isinstance(records, list):
        raise ValueError("artifact.records must be an array")
    for index, record in enumerate(records):
        _validate_record(
            record,
            index=index,
            split=split,
            candidate_ids=candidate_ids,
            bindings=bindings,
        )
    if not _records_have_expected_cardinality(artifact):
        raise ValueError("artifact record cardinality or uniqueness is invalid")
    aggregates = artifact["aggregates"]
    if not isinstance(aggregates, list):
        raise ValueError("artifact.aggregates must be an array")
    for index, aggregate in enumerate(aggregates):
        _validate_aggregate(aggregate, index=index)
    recomputed = _recompute_aggregates(artifact)
    if aggregates != recomputed:
        raise ValueError("artifact aggregate is stale or inconsistent")
    if split == "tuning":
        if artifact["decision"] is not None:
            raise ValueError("tuning artifact decision must be null")
        if winner_id != derive_tuning_winner(artifact):
            raise ValueError("artifact tuning winner is stale")
    else:
        if winner_id is None:
            raise ValueError("holdout artifact requires a frozen winner")
        decision = artifact["decision"]
        if decision not in {"PASS", "STOP"}:
            raise ValueError("holdout artifact decision is invalid")
        if decision != derive_holdout_decision(artifact):
            raise ValueError("artifact decision is stale")


def freeze_tuning_winner(payload: Mapping[str, Any]) -> dict[str, Any]:
    """Create a closed winner binding only after full tuning validation."""
    validate_artifact(payload, split="tuning")
    winner_id = payload["winner_id"]
    if not isinstance(winner_id, str):
        raise ValueError("validated tuning artifact has no eligible winner")
    candidates = [
        candidate
        for candidate in payload["candidates"]
        if candidate["id"] == winner_id
    ]
    if len(candidates) != 1:
        raise ValueError("validated tuning winner is not unique")
    return {
        "schema_version": 1,
        "winner_id": winner_id,
        "configuration_sha256": candidates[0]["configuration_sha256"],
        "config_sha256": payload["config_sha256"],
        "manifest_sha256": payload["manifest_sha256"],
        "tuning_artifact_sha256": _canonical_json_sha256(payload),
    }


def validate_frozen_winner(payload: Mapping[str, Any]) -> None:
    """Bind a frozen winner to canonical config, manifest, and candidate bytes."""
    winner = _closed_mapping(
        payload, fields=_FROZEN_WINNER_FIELDS, name="frozen_winner"
    )
    if winner["schema_version"] != 1 or not _is_int(winner["schema_version"]):
        raise ValueError("frozen_winner.schema_version must equal 1")
    winner_id = _string(winner["winner_id"], "frozen_winner.winner_id")
    bindings = _canonical_artifact_bindings()
    if (
        _sha256(winner["config_sha256"], "frozen_winner.config_sha256")
        != bindings.config_sha256
    ):
        raise ValueError("frozen winner config binding is not canonical")
    if (
        _sha256(winner["manifest_sha256"], "frozen_winner.manifest_sha256")
        != bindings.manifest_sha256
    ):
        raise ValueError("frozen winner manifest binding is not canonical")
    _sha256(
        winner["tuning_artifact_sha256"],
        "frozen_winner.tuning_artifact_sha256",
    )
    expected_candidates = _artifact_candidate_descriptors(load_config(DEFAULT_CONFIG))
    matches = [
        candidate
        for candidate in expected_candidates
        if candidate["id"] == winner_id
    ]
    if len(matches) != 1:
        raise ValueError("frozen winner ID is not canonical")
    if (
        _sha256(
            winner["configuration_sha256"],
            "frozen_winner.configuration_sha256",
        )
        != matches[0]["configuration_sha256"]
    ):
        raise ValueError("frozen winner configuration binding is not canonical")


def _record_bindings(
    bindings: ArtifactBindings, *, page_number: int
) -> dict[str, str]:
    render_sha256 = bindings.render_sha256_by_page.get(page_number)
    if render_sha256 is None:
        raise ValueError("page render is not canonically bound")
    return {
        "config_sha256": bindings.config_sha256,
        "manifest_sha256": bindings.manifest_sha256,
        "source_sha256": bindings.source_sha256,
        "render_sha256": render_sha256,
        "tesseract_sha256": bindings.tesseract_sha256,
        "tessdata_sha256": bindings.tessdata_sha256,
        "host_sha256": bindings.host_sha256,
        "toolchain_sha256": bindings.toolchain_sha256,
    }


def _empty_cell_evidence(annotation: PageAnnotation) -> list[dict[str, Any]]:
    if annotation.table is None:
        return []
    evidence: list[dict[str, Any]] = []
    for cell in annotation.table.cells:
        counts = error_counts(cell.text, "")
        evidence.append(
            {
                "row": cell.row,
                "column": cell.column,
                "character_edits": counts.character_edits,
                "reference_characters": counts.reference_characters,
                "word_edits": counts.word_edits,
                "reference_words": counts.reference_words,
                "reference_blank": cell.blank,
                "predicted_blank": False,
                "prediction_present": False,
            }
        )
    return evidence


def _content_evidence(
    annotation: PageAnnotation,
    recognition: GridRecognition,
) -> list[dict[str, Any]]:
    table = annotation.table
    if table is None:
        return []
    references = {
        (cell.row, cell.column): (cell.text, cell.blank) for cell in table.cells
    }
    predictions = {
        (cell.row, cell.column): cell.text for cell in recognition.cells
    }
    counts = measure_cell_content(
        references,
        predictions,
        reference_shape=(table.rows, table.columns),
        predicted_shape=(recognition.rows, recognition.columns),
    )
    evidence: list[dict[str, Any]] = []
    for coordinate, values in sorted(counts.cells.items()):
        (
            character_edits,
            reference_characters,
            word_edits,
            reference_words,
            reference_blank,
            predicted_blank,
        ) = values
        evidence.append(
            {
                "row": coordinate[0],
                "column": coordinate[1],
                "character_edits": character_edits,
                "reference_characters": reference_characters,
                "word_edits": word_edits,
                "reference_words": reference_words,
                "reference_blank": reference_blank,
                "predicted_blank": predicted_blank,
                "prediction_present": coordinate in predictions,
            }
        )
    return evidence


def _box_evidence(
    boxes: Sequence[tuple[int, int, Box]],
) -> list[dict[str, Any]]:
    return [
        {
            "row": row,
            "column": column,
            "bbox": [box.left, box.top, box.right, box.bottom],
        }
        for row, column, box in boxes
    ]


def _run_page(
    opened: OpenedPage,
    *,
    candidate: Mapping[str, Any],
    config: Mapping[str, Any],
    tessdata: Path,
    bindings: ArtifactBindings,
) -> dict[str, Any]:
    started = time.monotonic()
    annotation = opened.annotation
    image: Image.Image | None = None
    working: Image.Image | None = None
    status = "candidate_error"
    rows = columns = 0
    predicted_boxes: list[dict[str, Any]] = []
    peak_rss_bytes = 0
    cells = _empty_cell_evidence(annotation)
    try:
        image = Image.open(io.BytesIO(opened.render_bytes))
        image.load()
        detection = detect_ruled_table(
            image, detector_config(config, candidate["id"])
        )
        status = detection.status
        if detection.grid is not None:
            grid = detection.grid
            rows, columns = grid.rows, grid.columns
            predicted_boxes = _box_evidence(
                [
                    (cell.row, cell.column, cell.original_box)
                    for cell in grid.cells
                ]
            )
            table = annotation.table
            exact_grid = (
                table is not None
                and grid.rows == table.rows
                and grid.columns == table.columns
            )
            if exact_grid:
                working = prepare_working_image(
                    image, detection.deskew_angle_degrees
                )
                try:
                    recognition = recognize_grid(
                        grid,
                        working_image=working,
                        candidate_id=candidate["id"],
                        cell_inset_pixels=candidate["cell_inset_pixels"],
                        psm=candidate["psm"],
                        tessdata=tessdata,
                        limits=process_limits(),
                    )
                finally:
                    working.close()
                    working = None
                cells = _content_evidence(annotation, recognition)
                peak_rss_bytes = max(
                    (
                        int(cell.resource.get("peak_rss_bytes", 0))
                        for cell in recognition.cells
                    ),
                    default=0,
                )
    except TableRecognitionError as error:
        status = (
            "cleanup_error"
            if error.cleanup_failure is not None
            else error.error_kind
        )
        if status == "resource_limit":
            peak_rss_bytes = config["process_limits"]["max_rss_bytes"]
    except Exception:
        status = "candidate_error"
    finally:
        if working is not None:
            working.close()
        if image is not None:
            image.close()
    if status != "detected":
        rows = 0
        columns = 0
        predicted_boxes = []
    table = annotation.table
    reference_boxes = (
        _box_evidence(
            [
                (
                    cell.row,
                    cell.column,
                    Box(*cell.bbox),
                )
                for cell in table.cells
            ]
        )
        if table is not None
        else []
    )
    page_number = opened.page.page_number
    return {
        "id": f"{opened.page.split}:{candidate['id']}:{page_number}",
        "candidate_id": candidate["id"],
        "page_number": page_number,
        "split": opened.page.split,
        "negative": opened.page.negative,
        "status": status,
        "rows": rows,
        "columns": columns,
        "reference_rows": table.rows if table is not None else 0,
        "reference_columns": table.columns if table is not None else 0,
        "predicted_boxes": predicted_boxes,
        "reference_boxes": reference_boxes,
        "cells": cells,
        "elapsed_seconds": time.monotonic() - started,
        "peak_rss_bytes": peak_rss_bytes,
        "resource": {
            "timed_out": status == "timeout",
            "resource_violation": status == "resource_limit",
            "cleanup_failed": status == "cleanup_error",
        },
        "bindings": _record_bindings(bindings, page_number=page_number),
    }


def _bindings_for_run(
    manifest: CorpusManifest,
    *,
    config_path: Path,
    tessdata: Path,
    host: Mapping[str, Any],
    toolchain: Mapping[str, Any],
) -> ArtifactBindings:
    tesseract = shutil.which("tesseract")
    if tesseract is None:
        raise ValueError("Tesseract executable is unavailable")
    pages = manifest.tuning + manifest.holdout + manifest.negative
    return ArtifactBindings(
        config_sha256=_sha256_file(config_path),
        manifest_sha256=manifest.manifest_sha256,
        source_sha256=manifest.source_sha256,
        render_sha256_by_page={
            page.page_number: page.render_sha256 for page in pages
        },
        tesseract_sha256=_sha256_file(Path(tesseract)),
        tessdata_sha256=_sha256_file(tessdata / "vie.traineddata"),
        host_sha256=_canonical_json_sha256(host),
        toolchain_sha256=_canonical_json_sha256(toolchain),
    )


def _verify_annotation_readiness(
    manifest: CorpusManifest,
    annotations_path: Path,
    *,
    require_human_review: bool = True,
) -> int:
    pages = manifest.tuning + manifest.holdout + manifest.negative
    verified = sum(page.review_status == "human_verified" for page in pages)
    if require_human_review and verified != len(pages):
        raise ValueError(
            f"{verified}/{len(pages)} annotations are human_verified; "
            f"require {len(pages)}/{len(pages)} human_verified"
        )
    expected_directory = (manifest._root / "annotations").resolve()
    if annotations_path.resolve() != expected_directory:
        raise ValueError("annotations directory is not canonical for the manifest")
    for page in pages:
        annotation_path = annotations_path / Path(page.annotation_path).name
        raw = annotation_path.read_bytes()
        if hashlib.sha256(raw).hexdigest() != page.annotation_sha256:
            raise ValueError("annotation SHA-256 mismatch during preflight")
        try:
            payload = json.loads(raw)
        except (json.JSONDecodeError, UnicodeDecodeError) as error:
            raise ValueError("annotation is not valid UTF-8 JSON") from error
        annotation = _parse_annotation(
            payload,
            expected_render_sha256=page.render_sha256,
            require_verified_holdout=False,
        )
        if (
            annotation.source_sha256 != manifest.source_sha256
            or annotation.page_number != page.page_number
            or annotation.split != page.split
            or annotation.negative != page.negative
            or annotation.review_status != page.review_status
            or annotation.reviewer != page.reviewer
            or annotation.revision != page.revision
            or template_fingerprint(annotation) != page.template_fingerprint
        ):
            raise ValueError("annotation metadata does not match frozen manifest")
    return verified


def _verify_source_pdf(path: Path, config: Mapping[str, Any]) -> None:
    try:
        size = path.stat().st_size
    except OSError as error:
        raise ValueError("canonical source PDF is unavailable") from error
    if size != CANONICAL_SOURCE_SIZE:
        raise ValueError("canonical source PDF size mismatch")
    if _sha256_file(path) != config["source"]["expected_sha256"]:
        raise ValueError("canonical source PDF SHA-256 mismatch")


def _verify_tessdata(path: Path, config: Mapping[str, Any]) -> None:
    model = path / "vie.traineddata"
    if not path.is_dir() or not model.is_file():
        raise ValueError("canonical tessdata must contain vie.traineddata")
    if _sha256_file(model) != config["tessdata"]["vie_sha256"]:
        raise ValueError("canonical vie.traineddata SHA-256 mismatch")


def _preflight_official_inputs(
    *,
    config_path: Path,
    manifest_path: Path,
    annotations_path: Path,
    pdf_path: Path,
    tessdata: Path,
    split: Literal["tuning", "holdout"],
) -> tuple[dict[str, Any], CorpusManifest]:
    config = load_config(config_path)
    manifest = load_manifest(manifest_path, mode=split)
    _verify_annotation_readiness(
        manifest,
        annotations_path,
        require_human_review=True,
    )
    if manifest.manifest_sha256 != CANONICAL_MANIFEST_SHA256:
        raise ValueError("frozen manifest SHA-256 is not canonical")
    _verify_source_pdf(pdf_path, config)
    _verify_tessdata(tessdata, config)
    if manifest.access_counts != {"tuning": 0, "holdout": 0, "negative": 0}:
        raise ValueError("official preflight unexpectedly opened a page")
    return config, manifest


def run_split(
    split: Literal["tuning", "holdout"],
    *,
    manifest_path: Path = DEFAULT_MANIFEST,
    config_path: Path = DEFAULT_CONFIG,
    annotations_path: Path = DEFAULT_ANNOTATIONS,
    pdf_path: Path = DEFAULT_PDF,
    tessdata: Path = DEFAULT_TESSDATA,
    frozen_winner: Mapping[str, Any] | None = None,
) -> dict[str, Any]:
    """Run exactly one isolated split and return closed aggregate-only evidence."""
    if split not in {"tuning", "holdout"}:
        raise ValueError("split must be tuning or holdout")
    config, manifest = _preflight_official_inputs(
        config_path=config_path,
        manifest_path=manifest_path,
        annotations_path=annotations_path,
        pdf_path=pdf_path,
        tessdata=tessdata,
        split=split,
    )
    if split == "holdout":
        if frozen_winner is None:
            raise ValueError("holdout requires a frozen tuning winner artifact")
        validate_frozen_winner(frozen_winner)
        winner_id = frozen_winner["winner_id"]
        selected_candidates = [
            candidate
            for candidate in config["detector_candidates"]
            if candidate["id"] == winner_id
        ]
        pages = manifest.holdout + manifest.negative
    else:
        winner_id = None
        selected_candidates = list(config["detector_candidates"])
        pages = manifest.tuning
    opened_pages = [manifest.open_page(page.page_number) for page in pages]
    host = _host_description()
    toolchain = _toolchain_description()
    bindings = _bindings_for_run(
        manifest,
        config_path=config_path,
        tessdata=tessdata,
        host=host,
        toolchain=toolchain,
    )
    records = [
        _run_page(
            opened,
            candidate=candidate,
            config=config,
            tessdata=tessdata,
            bindings=bindings,
        )
        for candidate in selected_candidates
        for opened in opened_pages
    ]
    payload: dict[str, Any] = {
        "schema_version": 1,
        "split": split,
        "source": {
            "id": manifest.source_id,
            "sha256": manifest.source_sha256,
            "size_bytes": manifest.source_size_bytes,
        },
        "config_sha256": bindings.config_sha256,
        "manifest_sha256": bindings.manifest_sha256,
        "host": host,
        "toolchain": toolchain,
        "access": {
            "tuning_pages_opened": manifest.access_counts["tuning"],
            "holdout_pages_opened": manifest.access_counts["holdout"],
            "negative_pages_opened": manifest.access_counts["negative"],
        },
        "candidates": _artifact_candidate_descriptors(config),
        "records": records,
        "aggregates": [],
        "winner_id": winner_id,
        "decision": None,
    }
    payload["aggregates"] = _recompute_aggregates(payload)
    if split == "tuning":
        payload["winner_id"] = derive_tuning_winner(payload)
    else:
        payload["decision"] = derive_holdout_decision(payload)
    return payload


def _format_metric(value: Any) -> str:
    if isinstance(value, float):
        return f"{value:.6f}"
    return str(value)


def render_report(
    payload: Mapping[str, Any],
    holdout_payload: Mapping[str, Any] | None = None,
) -> str:
    """Render deterministic aggregate-only Markdown without raw records."""
    split = payload.get("split")
    if split not in {"tuning", "holdout"}:
        raise ValueError("artifact split is invalid")
    aggregates = payload.get("aggregates")
    if not isinstance(aggregates, list):
        raise ValueError("artifact aggregates are invalid")
    access = payload.get("access")
    source = payload.get("source")
    if not isinstance(access, Mapping) or not isinstance(source, Mapping):
        raise ValueError("artifact public provenance is invalid")
    lines = [
        "# Ruled-table OCR spike",
        "",
        "## Public provenance",
        f"- Source id: `{source.get('id')}`",
        f"- Source SHA-256: `{source.get('sha256')}`",
        f"- Manifest SHA-256: `{payload.get('manifest_sha256')}`",
        f"- Configuration SHA-256: `{payload.get('config_sha256')}`",
        f"- Split: `{'tuning+holdout' if holdout_payload is not None else split}`",
        (
            "- Tuning artifact access: "
            f"tuning={access.get('tuning_pages_opened')}, "
            f"holdout={access.get('holdout_pages_opened')}, "
            f"negative={access.get('negative_pages_opened')}"
        ),
        "",
        "## Bounds",
        "- Cell match IoU: `>= 0.80`",
        "- Cell F1: `>= 0.95`",
        "- Cell CER: `<= 0.05`",
        "- Empty-cell accuracy: `>= 0.98`",
        "- Peak RSS: `< 805306368` bytes",
        "- Page latency: `<= 20` seconds",
        "- Negative false positives: `0`",
        "",
        "## Candidate aggregates",
        (
            "| Candidate | Records | Exact grids | TP | FP | FN | Char edits / refs "
            "| Word edits / refs | F1 | CER | WER | Empty accuracy | Median s "
            "| Max s | Peak RSS | Failures | Negative FP |"
        ),
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
    ]
    if holdout_payload is not None:
        holdout_access = holdout_payload.get("access")
        if not isinstance(holdout_access, Mapping):
            raise ValueError("holdout artifact public provenance is invalid")
        lines.insert(
            9,
            (
                "- Holdout artifact access: "
                f"tuning={holdout_access.get('tuning_pages_opened')}, "
                f"holdout={holdout_access.get('holdout_pages_opened')}, "
                f"negative={holdout_access.get('negative_pages_opened')}"
            ),
        )
    for aggregate in sorted(aggregates, key=lambda item: item["candidate_id"]):
        lines.append(
            "| "
            + " | ".join(
                [
                    f"`{aggregate['candidate_id']}`",
                    str(aggregate["record_count"]),
                    str(aggregate["exact_grid_pages"]),
                    str(aggregate["cell_tp"]),
                    str(aggregate["cell_fp"]),
                    str(aggregate["cell_fn"]),
                    (
                        f"{aggregate['character_edits']} / "
                        f"{aggregate['reference_characters']}"
                    ),
                    (
                        f"{aggregate['word_edits']} / "
                        f"{aggregate['reference_words']}"
                    ),
                    _format_metric(aggregate["cell_f1"]),
                    _format_metric(aggregate["cell_cer"]),
                    _format_metric(aggregate["cell_wer"]),
                    _format_metric(aggregate["empty_cell_accuracy"]),
                    _format_metric(aggregate["median_page_latency_seconds"]),
                    _format_metric(aggregate["maximum_page_latency_seconds"]),
                    str(aggregate["peak_rss_bytes"]),
                    str(aggregate["failures"]),
                    str(aggregate["false_positives"]),
                ]
            )
            + " |"
        )
    winner = payload.get("winner_id")
    lines.extend(
        [
            "",
            "## Frozen tuning result",
            f"- Winner: `{winner}`" if winner is not None else "- Winner: none",
            f"- Frozen configuration SHA-256: `{payload.get('config_sha256')}`",
            "",
            "## Holdout gate",
            "| Condition | Measured | Threshold | Result |",
            "|---|---:|---:|---|",
        ]
    )
    gate_payload = holdout_payload if holdout_payload is not None else payload
    gate_aggregates = gate_payload.get("aggregates")
    if (
        gate_payload.get("split") == "holdout"
        and isinstance(gate_aggregates, list)
        and len(gate_aggregates) == 1
    ):
        aggregate = gate_aggregates[0]
        gate_rows = [
            (
                "Exact table grids",
                aggregate["exact_grid_pages"],
                "3 / 3",
                aggregate["exact_grid_pages"] == 3,
            ),
            ("Cell F1", aggregate["cell_f1"], ">= 0.95", aggregate["cell_f1"] >= 0.95),
            ("Cell CER", aggregate["cell_cer"], "<= 0.05", aggregate["cell_cer"] <= 0.05),
            (
                "Empty-cell accuracy",
                aggregate["empty_cell_accuracy"],
                ">= 0.98",
                aggregate["empty_cell_accuracy"] >= 0.98,
            ),
            (
                "Negative false positives",
                aggregate["false_positives"],
                "0",
                aggregate["false_positives"] == 0,
            ),
            (
                "Peak RSS bytes",
                aggregate["peak_rss_bytes"],
                "< 805306368",
                aggregate["peak_rss_bytes"] < 805_306_368,
            ),
            (
                "Maximum page latency seconds",
                aggregate["maximum_page_latency_seconds"],
                "<= 20",
                aggregate["maximum_page_latency_seconds"] <= 20,
            ),
        ]
        for label, measured, threshold, passed in gate_rows:
            lines.append(
                f"| {label} | {_format_metric(measured)} | {threshold} | "
                f"{'PASS' if passed else 'FAIL'} |"
            )
        decision = derive_holdout_decision(gate_payload)
    else:
        for label, threshold in (
            ("Exact table grids", "3 / 3"),
            ("Cell F1", ">= 0.95"),
            ("Cell CER", "<= 0.05"),
            ("Empty-cell accuracy", ">= 0.98"),
            ("Negative false positives", "0"),
            ("Peak RSS bytes", "< 805306368"),
            ("Maximum page latency seconds", "<= 20"),
        ):
            lines.append(f"| {label} | not measured | {threshold} | FAIL |")
        decision = "STOP"
    lines.extend(
        [
            "",
            f"## Decision: {decision}",
            "",
            "## Limitations",
            "- Supports one table per page with visible rules and no merged cells.",
            "- The corpus is intentionally tiny and is not representative of all documents.",
            "- This spike grants no production or full-document authorization.",
            "",
        ]
    )
    return "\n".join(lines)


def _load_json_artifact(path: Path) -> dict[str, Any]:
    try:
        payload = json.loads(path.read_bytes())
    except (json.JSONDecodeError, UnicodeDecodeError) as error:
        raise ValueError("artifact is not valid UTF-8 JSON") from error
    if not isinstance(payload, dict):
        raise ValueError("artifact must be an object")
    return payload


def _write_json_artifact(path: Path, payload: Mapping[str, Any]) -> None:
    encoded = (
        json.dumps(payload, ensure_ascii=False, sort_keys=True, indent=2) + "\n"
    ).encode("utf-8")
    _atomic_write_bytes(path, encoded)


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    inventory = subparsers.add_parser("inventory")
    inventory.add_argument("--manifest", type=Path, required=True)
    inventory.add_argument("--annotations", type=Path, required=True)
    inventory.add_argument("--output", type=Path, required=True)
    inventory.add_argument("--require-human-review", action="store_true")
    tune = subparsers.add_parser("tune")
    tune.add_argument("--config", type=Path, required=True)
    tune.add_argument("--manifest", type=Path, required=True)
    tune.add_argument("--annotations", type=Path, required=True)
    tune.add_argument("--pdf", type=Path, required=True)
    tune.add_argument("--tessdata", type=Path, required=True)
    tune.add_argument("--output", type=Path, required=True)
    holdout = subparsers.add_parser("holdout")
    holdout.add_argument("--config", type=Path, required=True)
    holdout.add_argument("--frozen-winner", type=Path, required=True)
    holdout.add_argument("--manifest", type=Path, required=True)
    holdout.add_argument("--annotations", type=Path, required=True)
    holdout.add_argument("--pdf", type=Path, required=True)
    holdout.add_argument("--tessdata", type=Path, required=True)
    holdout.add_argument("--output", type=Path, required=True)
    validate = subparsers.add_parser("validate")
    validate.add_argument("--input", type=Path, required=True)
    validate.add_argument("--split", choices=("tuning", "holdout"), required=True)
    report = subparsers.add_parser("report")
    report.add_argument("--tuning", type=Path, required=True)
    report.add_argument("--holdout", type=Path, required=True)
    report.add_argument("--output", type=Path, required=True)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    if args.command == "inventory":
        manifest = load_manifest(args.manifest, mode="tuning")
        _verify_annotation_readiness(
            manifest,
            args.annotations,
            require_human_review=args.require_human_review,
        )
        if manifest.manifest_sha256 != CANONICAL_MANIFEST_SHA256:
            raise ValueError("frozen manifest SHA-256 is not canonical")
        write_corpus_report(manifest, args.output)
    elif args.command == "tune":
        payload = run_split(
            "tuning",
            manifest_path=args.manifest,
            config_path=args.config,
            annotations_path=args.annotations,
            pdf_path=args.pdf,
            tessdata=args.tessdata,
        )
        _write_json_artifact(args.output, payload)
    elif args.command == "holdout":
        manifest = load_manifest(args.manifest, mode="holdout")
        _verify_annotation_readiness(
            manifest,
            args.annotations,
            require_human_review=True,
        )
        if manifest.manifest_sha256 != CANONICAL_MANIFEST_SHA256:
            raise ValueError("frozen manifest SHA-256 is not canonical")
        winner = _load_json_artifact(args.frozen_winner)
        payload = run_split(
            "holdout",
            manifest_path=args.manifest,
            config_path=args.config,
            annotations_path=args.annotations,
            pdf_path=args.pdf,
            tessdata=args.tessdata,
            frozen_winner=winner,
        )
        _write_json_artifact(args.output, payload)
    elif args.command == "validate":
        payload = _load_json_artifact(args.input)
        validate_artifact(
            payload,
            split=args.split,
        )
        if args.split == "tuning" and payload["winner_id"] is not None:
            frozen = freeze_tuning_winner(payload)
            _write_json_artifact(args.input.parent / "winner.json", frozen)
    elif args.command == "report":
        tuning = _load_json_artifact(args.tuning)
        validate_artifact(tuning, split="tuning")
        holdout_payload = (
            _load_json_artifact(args.holdout) if args.holdout is not None else None
        )
        if holdout_payload is not None:
            validate_artifact(holdout_payload, split="holdout")
        report = render_report(tuning, holdout_payload)
        _atomic_write_bytes(args.output, report.encode("utf-8"))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
