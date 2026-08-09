#!/usr/bin/env python3
"""Bounded, Pillow-only ruled-table geometry for the OCR experiment."""

from __future__ import annotations

import math
import statistics
from dataclasses import dataclass
from types import MappingProxyType
from typing import Literal, Mapping, Sequence

from PIL import Image


DetectionStatus = Literal[
    "detected", "not_detected", "unsupported", "invalid_grid"
]
_MAX_PIXELS = 50_000_000
_MAX_DIMENSION = 10_000
_MAX_ROWS = 50
_MAX_COLUMNS = 30
_MAX_CELLS = 1_500
_SCORE_LONGEST_SIDE = 1_000


def _is_int(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


@dataclass(frozen=True, slots=True)
class DetectorConfig:
    dark_max: int
    min_horizontal_fraction: float
    min_vertical_fraction: float
    max_gap_pixels: int
    cluster_tolerance_pixels: int
    intersection_tolerance_pixels: int
    deskew_angles_degrees: tuple[float, ...]
    max_rows: int
    max_columns: int
    max_cells: int
    max_pixels: int
    max_dimension: int

    def __post_init__(self) -> None:
        integer_limits = {
            "dark_max": (self.dark_max, 1, 255),
            "max_gap_pixels": (self.max_gap_pixels, 0, _MAX_DIMENSION),
            "cluster_tolerance_pixels": (
                self.cluster_tolerance_pixels,
                0,
                _MAX_DIMENSION,
            ),
            "intersection_tolerance_pixels": (
                self.intersection_tolerance_pixels,
                0,
                _MAX_DIMENSION,
            ),
            "max_rows": (self.max_rows, 1, _MAX_ROWS),
            "max_columns": (self.max_columns, 1, _MAX_COLUMNS),
            "max_cells": (self.max_cells, 1, _MAX_CELLS),
            "max_pixels": (self.max_pixels, 1, _MAX_PIXELS),
            "max_dimension": (self.max_dimension, 1, _MAX_DIMENSION),
        }
        for name, (value, minimum, maximum) in integer_limits.items():
            if not _is_int(value) or not minimum <= value <= maximum:
                raise ValueError(
                    f"{name} must be an integer between {minimum} and {maximum}"
                )
        for name, value in (
            ("min_horizontal_fraction", self.min_horizontal_fraction),
            ("min_vertical_fraction", self.min_vertical_fraction),
        ):
            if (
                isinstance(value, bool)
                or not isinstance(value, (int, float))
                or not math.isfinite(float(value))
                or not 0 < float(value) <= 1
            ):
                raise ValueError(f"{name} must be a finite fraction")
        angles = self.deskew_angles_degrees
        if (
            not isinstance(angles, tuple)
            or not angles
            or any(
                isinstance(angle, bool)
                or not isinstance(angle, (int, float))
                or not math.isfinite(float(angle))
                for angle in angles
            )
            or tuple(sorted(float(angle) for angle in angles))
            != tuple(float(angle) for angle in angles)
            or len(set(float(angle) for angle in angles)) != len(angles)
            or 0.0 not in angles
            or any(abs(float(angle)) > 5.0 for angle in angles)
        ):
            raise ValueError(
                "deskew_angles_degrees must be a sorted unique tuple including 0"
            )
        object.__setattr__(
            self,
            "deskew_angles_degrees",
            tuple(float(angle) for angle in angles),
        )
        if self.max_rows * self.max_columns > self.max_cells:
            raise ValueError(
                "max_rows * max_columns must not exceed max_cells"
            )


@dataclass(frozen=True, slots=True)
class Box:
    left: int
    top: int
    right: int
    bottom: int

    def __post_init__(self) -> None:
        if any(
            not _is_int(value)
            for value in (self.left, self.top, self.right, self.bottom)
        ):
            raise ValueError("box coordinates must be integers")
        if (
            self.left < 0
            or self.top < 0
            or self.right <= self.left
            or self.bottom <= self.top
        ):
            raise ValueError("box must have positive area and nonnegative origin")

    @property
    def area(self) -> int:
        return max(0, self.right - self.left) * max(
            0, self.bottom - self.top
        )


@dataclass(frozen=True, slots=True)
class GridCell:
    row: int
    column: int
    working_box: Box
    original_box: Box

    def __post_init__(self) -> None:
        if (
            not _is_int(self.row)
            or not _is_int(self.column)
            or self.row < 0
            or self.column < 0
        ):
            raise ValueError("cell coordinates must be nonnegative integers")

    @property
    def coordinate(self) -> tuple[int, int]:
        return self.row, self.column


@dataclass(frozen=True, slots=True)
class Grid:
    rows: int
    columns: int
    working_table_box: Box
    original_table_box: Box
    cells: tuple[GridCell, ...]

    def __post_init__(self) -> None:
        if not _is_int(self.rows) or not 1 <= self.rows <= _MAX_ROWS:
            raise ValueError("rows exceeds max_rows")
        if not _is_int(self.columns) or not 1 <= self.columns <= _MAX_COLUMNS:
            raise ValueError("columns exceeds max_columns")
        if not isinstance(self.cells, tuple):
            raise ValueError("cells must be an immutable tuple")
        if len(self.cells) > _MAX_CELLS:
            raise ValueError("cells exceeds max_cells")
        expected_count = self.rows * self.columns
        if expected_count > _MAX_CELLS:
            raise ValueError("grid exceeds max_cells")
        if len(self.cells) != expected_count:
            raise ValueError("cells must form a complete rectangular grid")
        expected = tuple(
            (row, column)
            for row in range(self.rows)
            for column in range(self.columns)
        )
        actual = tuple(cell.coordinate for cell in self.cells)
        if actual != expected or len(set(actual)) != len(actual):
            raise ValueError("cells must be unique and in row-major order")
        for cell in self.cells:
            if not _box_within(cell.working_box, self.working_table_box):
                raise ValueError("working cell lies outside table box")
            if not _box_within(cell.original_box, self.original_table_box):
                raise ValueError("original cell lies outside table box")


@dataclass(frozen=True, slots=True)
class DetectionResult:
    status: DetectionStatus
    deskew_angle_degrees: float
    working_size: tuple[int, int]
    grid: Grid | None
    diagnostics: Mapping[str, int | float | str]

    def __post_init__(self) -> None:
        if self.status not in {
            "detected",
            "not_detected",
            "unsupported",
            "invalid_grid",
        }:
            raise ValueError("unsupported detection status")
        if (
            isinstance(self.deskew_angle_degrees, bool)
            or not isinstance(self.deskew_angle_degrees, (int, float))
            or not math.isfinite(float(self.deskew_angle_degrees))
        ):
            raise ValueError("deskew angle must be finite")
        if (
            not isinstance(self.working_size, tuple)
            or len(self.working_size) != 2
            or any(not _is_int(value) or value < 0 for value in self.working_size)
        ):
            raise ValueError("working_size must contain nonnegative integers")
        if (self.status == "detected") != (self.grid is not None):
            raise ValueError("only detected results may carry a grid")
        if not isinstance(self.diagnostics, Mapping):
            raise ValueError("diagnostics must be a mapping")
        diagnostics = dict(self.diagnostics)
        if any(not isinstance(key, str) for key in diagnostics) or any(
            isinstance(value, bool)
            or not isinstance(value, (int, float, str))
            for value in diagnostics.values()
        ):
            raise ValueError("diagnostics must contain scalar counts and geometry")
        object.__setattr__(
            self, "deskew_angle_degrees", float(self.deskew_angle_degrees)
        )
        object.__setattr__(
            self, "diagnostics", MappingProxyType(diagnostics)
        )


@dataclass(frozen=True, slots=True)
class _Run:
    coordinate: int
    start: int
    end: int


@dataclass(frozen=True, slots=True)
class _Line:
    coordinate: int
    segments: tuple[tuple[int, int], ...]


def _box_within(inner: Box, outer: Box) -> bool:
    return (
        outer.left <= inner.left < inner.right <= outer.right
        and outer.top <= inner.top < inner.bottom <= outer.bottom
    )


def prepare_working_image(
    image: Image.Image, angle_degrees: float
) -> Image.Image:
    """Return a new grayscale image using the detector's exact rotation."""
    if (
        isinstance(angle_degrees, bool)
        or not isinstance(angle_degrees, (int, float))
        or not math.isfinite(float(angle_degrees))
    ):
        raise ValueError("angle_degrees must be finite")
    grayscale = image.convert("L")
    if float(angle_degrees) == 0.0:
        return grayscale
    try:
        return grayscale.rotate(
            float(angle_degrees),
            resample=Image.Resampling.BICUBIC,
            expand=False,
            fillcolor=255,
        )
    finally:
        grayscale.close()


def _binary_bytes(image: Image.Image, dark_max: int) -> bytes:
    mask = image.point(tuple(1 if value <= dark_max else 0 for value in range(256)))
    try:
        return mask.tobytes()
    finally:
        mask.close()


def _longest_dark_run(
    mask: bytes,
    start: int,
    stop: int,
    stride: int,
    max_gap: int,
) -> int:
    longest = 0
    cursor = start
    while cursor < stop:
        while cursor < stop and mask[cursor] == 0:
            cursor += stride
        if cursor >= stop:
            break
        run_start = cursor
        last_dark = cursor
        gap = 0
        cursor += stride
        while cursor < stop:
            if mask[cursor]:
                last_dark = cursor
                gap = 0
            else:
                gap += 1
                if gap > max_gap:
                    break
            cursor += stride
        longest = max(longest, (last_dark - run_start) // stride + 1)
    return longest


def _angle_score(
    image: Image.Image, dark_max: int, max_gap: int
) -> int:
    mask = _binary_bytes(image, dark_max)
    width, height = image.size
    horizontal = [
        _longest_dark_run(mask, row * width, (row + 1) * width, 1, max_gap)
        for row in range(height)
    ]
    vertical = [
        _longest_dark_run(mask, column, width * height, width, max_gap)
        for column in range(width)
    ]
    horizontal.sort(reverse=True)
    vertical.sort(reverse=True)
    return sum(horizontal[:3]) + sum(vertical[:3])


def _choose_deskew_angle(
    image: Image.Image, config: DetectorConfig
) -> tuple[float, int, tuple[int, int]]:
    width, height = image.size
    scale = min(1.0, _SCORE_LONGEST_SIDE / max(width, height))
    score_size = (
        max(1, round(width * scale)),
        max(1, round(height * scale)),
    )
    sampled = image.resize(score_size, resample=Image.Resampling.LANCZOS)
    grayscale = sampled.convert("L")
    sampled.close()
    scaled_gap = max(0, round(config.max_gap_pixels * scale))
    scored: list[tuple[int, float]] = []
    try:
        for angle in config.deskew_angles_degrees:
            rotated = prepare_working_image(grayscale, angle)
            try:
                scored.append(
                    (
                        _angle_score(rotated, config.dark_max, scaled_gap),
                        angle,
                    )
                )
            finally:
                rotated.close()
    finally:
        grayscale.close()
    score, angle = min(
        scored,
        key=lambda item: (-item[0], abs(item[1]), item[1]),
    )
    return angle, score, score_size


def _scan_runs(
    mask: bytes,
    *,
    width: int,
    height: int,
    horizontal: bool,
    minimum_length: int,
    max_gap: int,
) -> list[_Run]:
    line_count = height if horizontal else width
    line_length = width if horizontal else height
    stride = 1 if horizontal else width
    runs: list[_Run] = []
    for coordinate in range(line_count):
        line_start = coordinate * width if horizontal else coordinate
        cursor = 0
        while cursor < line_length:
            index = line_start + cursor * stride
            while cursor < line_length and mask[index] == 0:
                cursor += 1
                index += stride
            if cursor >= line_length:
                break
            run_start = cursor
            last_dark = cursor
            gap = 0
            cursor += 1
            while cursor < line_length:
                index = line_start + cursor * stride
                if mask[index]:
                    last_dark = cursor
                    gap = 0
                else:
                    gap += 1
                    if gap > max_gap:
                        break
                cursor += 1
            if last_dark - run_start + 1 >= minimum_length:
                runs.append(_Run(coordinate, run_start, last_dark))
    return runs


def _merge_same_coordinate(runs: Sequence[_Run]) -> list[_Run]:
    merged: list[_Run] = []
    for run in sorted(runs, key=lambda item: (item.coordinate, item.start, item.end)):
        if (
            merged
            and merged[-1].coordinate == run.coordinate
            and run.start <= merged[-1].end
        ):
            previous = merged[-1]
            merged[-1] = _Run(
                previous.coordinate, previous.start, max(previous.end, run.end)
            )
        else:
            merged.append(run)
    return merged


def _cluster_lines(
    runs: Sequence[_Run], tolerance: int
) -> tuple[_Line, ...]:
    by_coordinate: dict[int, list[_Run]] = {}
    for run in runs:
        by_coordinate.setdefault(run.coordinate, []).append(run)
    coordinates = sorted(by_coordinate)
    clusters: list[list[int]] = []
    for coordinate in coordinates:
        if clusters and coordinate - clusters[-1][-1] <= tolerance:
            clusters[-1].append(coordinate)
        else:
            clusters.append([coordinate])

    lines: list[_Line] = []
    for cluster in clusters:
        coordinate = int(round(statistics.median(cluster)))
        intervals = sorted(
            (run.start, run.end)
            for member in cluster
            for run in by_coordinate[member]
        )
        segments: list[tuple[int, int]] = []
        for start, end in intervals:
            if segments and start <= segments[-1][1] + 1:
                segments[-1] = (segments[-1][0], max(segments[-1][1], end))
            else:
                segments.append((start, end))
        lines.append(_Line(coordinate, tuple(segments)))
    return tuple(lines)


def _covers_point(line: _Line, point: int, tolerance: int) -> bool:
    return any(
        start - tolerance <= point <= end + tolerance
        for start, end in line.segments
    )


def _covers_interval(
    line: _Line, start: int, end: int, tolerance: int
) -> bool:
    return any(
        segment_start <= start + tolerance
        and segment_end >= end - tolerance
        for segment_start, segment_end in line.segments
    )


def _complete_regions(
    horizontal: Sequence[_Line],
    vertical: Sequence[_Line],
    tolerance: int,
) -> tuple[tuple[tuple[int, ...], tuple[int, ...]], ...]:
    candidates: set[tuple[tuple[int, ...], tuple[int, ...]]] = set()
    for top_index in range(len(horizontal) - 2):
        top = horizontal[top_index]
        for bottom_index in range(top_index + 2, len(horizontal)):
            bottom = horizontal[bottom_index]
            y1, y2 = top.coordinate, bottom.coordinate
            for left_index in range(len(vertical) - 2):
                left = vertical[left_index]
                for right_index in range(left_index + 2, len(vertical)):
                    right = vertical[right_index]
                    x1, x2 = left.coordinate, right.coordinate
                    if not (
                        _covers_interval(top, x1, x2, tolerance)
                        and _covers_interval(bottom, x1, x2, tolerance)
                        and _covers_interval(left, y1, y2, tolerance)
                        and _covers_interval(right, y1, y2, tolerance)
                    ):
                        continue
                    h_indices = tuple(
                        index
                        for index in range(top_index, bottom_index + 1)
                        if _covers_interval(
                            horizontal[index], x1, x2, tolerance
                        )
                    )
                    v_indices = tuple(
                        index
                        for index in range(left_index, right_index + 1)
                        if _covers_interval(vertical[index], y1, y2, tolerance)
                    )
                    if (
                        len(h_indices) < 3
                        or len(v_indices) < 3
                        or h_indices[0] != top_index
                        or h_indices[-1] != bottom_index
                        or v_indices[0] != left_index
                        or v_indices[-1] != right_index
                    ):
                        continue
                    if all(
                        _covers_point(
                            horizontal[row],
                            vertical[column].coordinate,
                            tolerance,
                        )
                        and _covers_point(
                            vertical[column],
                            horizontal[row].coordinate,
                            tolerance,
                        )
                        for row in h_indices
                        for column in v_indices
                    ):
                        candidates.add((h_indices, v_indices))

    maximal: list[tuple[tuple[int, ...], tuple[int, ...]]] = []
    for candidate in sorted(candidates):
        h_indices, v_indices = candidate
        h_set = set(h_indices)
        v_set = set(v_indices)
        if any(
            candidate != other
            and h_set.issubset(other[0])
            and v_set.issubset(other[1])
            for other in candidates
        ):
            continue
        maximal.append(candidate)
    return tuple(maximal)


def _inverse_box(
    box: Box,
    *,
    angle_degrees: float,
    size: tuple[int, int],
) -> Box:
    width, height = size
    radians = math.radians(angle_degrees)
    cosine = math.cos(radians)
    sine = math.sin(radians)
    center_x = width / 2.0
    center_y = height / 2.0
    points = []
    for x, y in (
        (box.left, box.top),
        (box.right, box.top),
        (box.right, box.bottom),
        (box.left, box.bottom),
    ):
        original_x = (
            cosine * (x - center_x) - sine * (y - center_y) + center_x
        )
        original_y = (
            sine * (x - center_x) + cosine * (y - center_y) + center_y
        )
        points.append((original_x, original_y))
    left = max(0, min(width - 1, math.floor(min(x for x, _ in points))))
    top = max(0, min(height - 1, math.floor(min(y for _, y in points))))
    right = max(left + 1, min(width, math.ceil(max(x for x, _ in points))))
    bottom = max(top + 1, min(height, math.ceil(max(y for _, y in points))))
    return Box(left, top, right, bottom)


def _result(
    status: DetectionStatus,
    *,
    angle: float,
    size: tuple[int, int],
    grid: Grid | None = None,
    diagnostics: Mapping[str, int | float | str],
) -> DetectionResult:
    return DetectionResult(status, angle, size, grid, diagnostics)


def detect_ruled_table(
    image: Image.Image, config: DetectorConfig
) -> DetectionResult:
    """Detect one complete rectangular ruled grid without retaining image data."""
    if not isinstance(image, Image.Image):
        raise TypeError("image must be a Pillow Image")
    if not isinstance(config, DetectorConfig):
        raise TypeError("config must be DetectorConfig")
    width, height = image.size
    base_diagnostics: dict[str, int | float | str] = {
        "width": width,
        "height": height,
        "cells_allocated": 0,
    }
    if width <= 0 or height <= 0:
        return _result(
            "invalid_grid",
            angle=0.0,
            size=(width, height),
            diagnostics={**base_diagnostics, "limit": "positive_dimensions"},
        )
    if width > config.max_dimension or height > config.max_dimension:
        return _result(
            "invalid_grid",
            angle=0.0,
            size=(width, height),
            diagnostics={**base_diagnostics, "limit": "max_dimension"},
        )
    if width * height > config.max_pixels:
        return _result(
            "invalid_grid",
            angle=0.0,
            size=(width, height),
            diagnostics={**base_diagnostics, "limit": "max_pixels"},
        )

    angle, score, score_size = _choose_deskew_angle(image, config)
    working = prepare_working_image(image, angle)
    try:
        mask = _binary_bytes(working, config.dark_max)
        horizontal_runs = _merge_same_coordinate(
            _scan_runs(
                mask,
                width=width,
                height=height,
                horizontal=True,
                minimum_length=math.ceil(
                    width * config.min_horizontal_fraction
                ),
                max_gap=config.max_gap_pixels,
            )
        )
        vertical_runs = _merge_same_coordinate(
            _scan_runs(
                mask,
                width=width,
                height=height,
                horizontal=False,
                minimum_length=math.ceil(
                    height * config.min_vertical_fraction
                ),
                max_gap=config.max_gap_pixels,
            )
        )
        horizontal = _cluster_lines(
            horizontal_runs, config.cluster_tolerance_pixels
        )
        vertical = _cluster_lines(
            vertical_runs, config.cluster_tolerance_pixels
        )
    finally:
        working.close()

    diagnostics = {
        **base_diagnostics,
        "deskew_score": score,
        "score_width": score_size[0],
        "score_height": score_size[1],
        "horizontal_runs": len(horizontal_runs),
        "vertical_runs": len(vertical_runs),
        "horizontal_lines": len(horizontal),
        "vertical_lines": len(vertical),
    }
    if len(horizontal) - 1 > config.max_rows:
        return _result(
            "invalid_grid",
            angle=angle,
            size=(width, height),
            diagnostics={**diagnostics, "limit": "max_rows"},
        )
    if len(vertical) - 1 > config.max_columns:
        return _result(
            "invalid_grid",
            angle=angle,
            size=(width, height),
            diagnostics={**diagnostics, "limit": "max_columns"},
        )
    possible_cells = max(0, len(horizontal) - 1) * max(
        0, len(vertical) - 1
    )
    if possible_cells > config.max_cells:
        return _result(
            "invalid_grid",
            angle=angle,
            size=(width, height),
            diagnostics={**diagnostics, "limit": "max_cells"},
        )

    regions = _complete_regions(
        horizontal, vertical, config.intersection_tolerance_pixels
    )
    diagnostics["complete_regions"] = len(regions)
    if len(regions) > 1:
        return _result(
            "unsupported",
            angle=angle,
            size=(width, height),
            diagnostics=diagnostics,
        )
    if not regions:
        incomplete_candidate = (
            len(horizontal) >= 2
            and len(vertical) >= 2
            and (len(horizontal) >= 3 or len(vertical) >= 3)
        )
        return _result(
            "invalid_grid" if incomplete_candidate else "not_detected",
            angle=angle,
            size=(width, height),
            diagnostics=diagnostics,
        )

    h_indices, v_indices = regions[0]
    rows = len(h_indices) - 1
    columns = len(v_indices) - 1
    cell_count = rows * columns
    if rows > config.max_rows:
        limit = "max_rows"
    elif columns > config.max_columns:
        limit = "max_columns"
    elif cell_count > config.max_cells:
        limit = "max_cells"
    else:
        limit = ""
    if limit:
        return _result(
            "invalid_grid",
            angle=angle,
            size=(width, height),
            diagnostics={**diagnostics, "limit": limit},
        )

    x_coordinates = tuple(vertical[index].coordinate for index in v_indices)
    y_coordinates = tuple(horizontal[index].coordinate for index in h_indices)
    if any(
        left >= right
        for left, right in zip(x_coordinates, x_coordinates[1:])
    ) or any(top >= bottom for top, bottom in zip(y_coordinates, y_coordinates[1:])):
        return _result(
            "invalid_grid",
            angle=angle,
            size=(width, height),
            diagnostics={**diagnostics, "limit": "increasing_coordinates"},
        )
    working_table_box = Box(
        x_coordinates[0],
        y_coordinates[0],
        x_coordinates[-1],
        y_coordinates[-1],
    )
    original_table_box = _inverse_box(
        working_table_box, angle_degrees=angle, size=(width, height)
    )
    cells: list[GridCell] = []
    for row in range(rows):
        for column in range(columns):
            working_box = Box(
                x_coordinates[column],
                y_coordinates[row],
                x_coordinates[column + 1],
                y_coordinates[row + 1],
            )
            cells.append(
                GridCell(
                    row,
                    column,
                    working_box,
                    _inverse_box(
                        working_box,
                        angle_degrees=angle,
                        size=(width, height),
                    ),
                )
            )
    diagnostics["cells_allocated"] = len(cells)
    grid = Grid(
        rows,
        columns,
        working_table_box,
        original_table_box,
        tuple(cells),
    )
    return _result(
        "detected",
        angle=angle,
        size=(width, height),
        grid=grid,
        diagnostics=diagnostics,
    )
