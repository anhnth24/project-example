#!/usr/bin/env python3
"""Frozen corpus contracts for the benchmark-only ruled-table OCR spike."""

from __future__ import annotations

import hashlib
import json
import os
import re
import tempfile
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path, PurePosixPath
from typing import Any, Literal, Mapping, Sequence

from benchmark.render import RenderLimits, open_pdf, render_page


SERVICE_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CONFIG = SERVICE_ROOT / "experiments" / "ruled-table-config.json"
CANONICAL_CONFIG_SHA256 = (
    "521efe33c8e128581708c6269e92486799201f59c648f6117048735530b0a495"
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
        "geometry_limits",
        "process_limits",
        "detector_candidates",
        "gate",
    }
)
_SOURCE_CONFIG_FIELDS = frozenset({"id", "expected_sha256", "max_bytes"})
_RENDER_FIELDS = frozenset({"dpi", "max_pixels", "max_dimension"})
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
