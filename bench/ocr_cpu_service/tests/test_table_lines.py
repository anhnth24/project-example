from __future__ import annotations

import random
import sys
from dataclasses import FrozenInstanceError, fields
from pathlib import Path

import pytest
from PIL import Image, ImageDraw

sys.path.insert(0, str(Path(__file__).parents[1]))

from experiments.table_lines import (  # noqa: E402
    Box,
    DetectionResult,
    DetectorConfig,
    Grid,
    GridCell,
    detect_ruled_table,
    prepare_working_image,
)
from experiments.ruled_table import detector_config  # noqa: E402


SERVICE_ROOT = Path(__file__).parents[1]
CONFIG = SERVICE_ROOT / "experiments" / "ruled-table-config.json"


def balanced_config(**overrides: object) -> DetectorConfig:
    values: dict[str, object] = {
        "dark_max": 160,
        "min_horizontal_fraction": 0.2,
        "min_vertical_fraction": 0.08,
        "max_gap_pixels": 8,
        "cluster_tolerance_pixels": 3,
        "intersection_tolerance_pixels": 3,
        "deskew_angles_degrees": (-1.5, -1.0, -0.5, 0.0, 0.5, 1.0, 1.5),
        "max_rows": 50,
        "max_columns": 30,
        "max_cells": 1_500,
        "max_pixels": 50_000_000,
        "max_dimension": 10_000,
    }
    values.update(overrides)
    return DetectorConfig(**values)


def grid_image(
    *,
    width: int = 240,
    height: int = 160,
    xs: tuple[int, ...] = (20, 120, 220),
    ys: tuple[int, ...] = (20, 80, 140),
) -> Image.Image:
    image = Image.new("L", (width, height), 255)
    draw = ImageDraw.Draw(image)
    for x in xs:
        draw.line((x, ys[0], x, ys[-1]), fill=0, width=2)
    for y in ys:
        draw.line((xs[0], y, xs[-1], y), fill=0, width=2)
    return image


def dotted_leader_fixture() -> Image.Image:
    image = Image.new("L", (240, 160), 255)
    draw = ImageDraw.Draw(image)
    for y in (30, 80, 130):
        for x in range(20, 221, 12):
            draw.line((x, y, x + 3, y), fill=0, width=2)
    return image


def two_grid_fixture() -> Image.Image:
    image = Image.new("L", (320, 220), 255)
    draw = ImageDraw.Draw(image)
    for xs, ys in (
        ((10, 60, 110), (10, 50, 90)),
        ((190, 245, 300), (125, 165, 205)),
    ):
        for x in xs:
            draw.line((x, ys[0], x, ys[-1]), fill=0, width=2)
        for y in ys:
            draw.line((xs[0], y, xs[-1], y), fill=0, width=2)
    return image


def gapped_grid_fixture(gap: int) -> Image.Image:
    image = grid_image()
    draw = ImageDraw.Draw(image)
    for y in (20, 80, 140):
        start = 65
        draw.rectangle((start, y, start + gap - 1, y + 1), fill=255)
    return image


def incomplete_intersection_fixture() -> Image.Image:
    image = grid_image()
    ImageDraw.Draw(image).rectangle((116, 76, 125, 85), fill=255)
    return image


def merged_cell_fixture() -> Image.Image:
    image = grid_image()
    ImageDraw.Draw(image).rectangle((116, 23, 125, 77), fill=255)
    return image


def merged_subgrid_fixture() -> Image.Image:
    image = grid_image(
        xs=(20, 70, 120, 170, 220),
        ys=(20, 50, 80, 110, 140),
    )
    ImageDraw.Draw(image).rectangle((115, 75, 126, 86), fill=255)
    return image


def _regular_coordinates(count: int, *, start: int = 10, step: int = 12) -> tuple[int, ...]:
    return tuple(start + index * step for index in range(count))


def _iou(left: Box, right: Box) -> float:
    intersection = max(
        0, min(left.right, right.right) - max(left.left, right.left)
    ) * max(0, min(left.bottom, right.bottom) - max(left.top, right.top))
    union = left.area + right.area - intersection
    return intersection / union


def test_detects_exact_two_by_two_grid():
    with grid_image() as image:
        result = detect_ruled_table(image, balanced_config())
    assert result.status == "detected"
    assert result.grid is not None
    assert (result.grid.rows, result.grid.columns) == (2, 2)
    assert [cell.coordinate for cell in result.grid.cells] == [
        (0, 0),
        (0, 1),
        (1, 0),
        (1, 1),
    ]
    assert result.deskew_angle_degrees == 0.0
    assert result.working_size == (240, 160)


def test_dotted_leader_without_vertical_intersections_is_not_a_table():
    with dotted_leader_fixture() as image:
        result = detect_ruled_table(image, balanced_config())
    assert result.status == "not_detected"


def test_two_valid_grids_are_unsupported():
    with two_grid_fixture() as image:
        result = detect_ruled_table(image, balanced_config())
    assert result.status == "unsupported"
    assert result.grid is None


def test_segment_gap_at_configured_maximum_is_bridged():
    with gapped_grid_fixture(8) as image:
        result = detect_ruled_table(image, balanced_config())
    assert result.status == "detected"


def test_segment_gap_beyond_configured_maximum_is_invalid_grid():
    with gapped_grid_fixture(9) as image:
        result = detect_ruled_table(image, balanced_config())
    assert result.status == "invalid_grid"


def test_isolated_noise_does_not_change_grid():
    with grid_image() as image:
        draw = ImageDraw.Draw(image)
        for point in ((2, 2), (235, 7), (8, 155), (230, 153), (50, 50)):
            draw.point(point, fill=0)
        result = detect_ruled_table(image, balanced_config())
    assert result.status == "detected"
    assert result.grid is not None
    assert (result.grid.rows, result.grid.columns) == (2, 2)


def test_connected_partial_segment_inside_cell_invalidates_grid_component():
    with grid_image() as image:
        ImageDraw.Draw(image).line((20, 50, 100, 50), fill=0, width=2)
        result = detect_ruled_table(image, balanced_config())
    assert result.status == "invalid_grid"
    assert result.grid is None


def test_partial_internal_line_cannot_shrink_component_to_three_by_three():
    with grid_image(
        xs=(20, 80, 140, 220),
        ys=(20, 60, 100, 140),
    ) as image:
        ImageDraw.Draw(image).line((20, 40, 70, 40), fill=0, width=2)
        result = detect_ruled_table(image, balanced_config())
    assert result.status == "invalid_grid"
    assert result.grid is None


def test_open_rule_overhang_cannot_be_accepted_as_smaller_three_by_three():
    image = Image.new("L", (240, 160), 255)
    draw = ImageDraw.Draw(image)
    for x in (20, 80, 140, 180):
        draw.line((x, 20, x, 140), fill=0, width=2)
    for y in (20, 60, 100, 140):
        draw.line((20, y, 220, y), fill=0, width=2)
    try:
        result = detect_ruled_table(image, balanced_config())
    finally:
        image.close()
    assert result.status == "invalid_grid"
    assert result.grid is None


@pytest.mark.parametrize("input_angle", (-1.5, 1.5))
def test_deskew_recovers_rotated_grid_and_inverse_maps_cells(input_angle):
    with grid_image() as source:
        rotated = source.rotate(
            input_angle,
            resample=Image.Resampling.BICUBIC,
            expand=False,
            fillcolor=255,
        )
    try:
        result = detect_ruled_table(rotated, balanced_config())
        assert result.status == "detected"
        assert result.grid is not None
        assert result.deskew_angle_degrees == -input_angle
        expected = (
            Box(20, 20, 120, 80),
            Box(120, 20, 220, 80),
            Box(20, 80, 120, 140),
            Box(120, 80, 220, 140),
        )
        assert all(
            _iou(cell.original_box, fixture_box) >= 0.80
            for cell, fixture_box in zip(result.grid.cells, expected, strict=True)
        )
    finally:
        rotated.close()


def test_blank_cells_do_not_affect_geometry():
    with grid_image() as image:
        result = detect_ruled_table(image, balanced_config())
    assert result.status == "detected"
    assert result.grid is not None
    assert len(result.grid.cells) == 4


@pytest.mark.parametrize(
    "fixture",
    (
        incomplete_intersection_fixture,
        merged_cell_fixture,
        merged_subgrid_fixture,
    ),
)
def test_incomplete_or_merged_like_grid_is_typed_invalid(fixture):
    with fixture() as image:
        result = detect_ruled_table(image, balanced_config())
    assert result.status == "invalid_grid"
    assert result.grid is None


@pytest.mark.parametrize(
    ("xs", "ys", "size", "expected_limit"),
    [
        (
            (20, 120, 220),
            _regular_coordinates(52),
            (240, 640),
            "max_rows",
        ),
        (
            _regular_coordinates(32),
            (20, 80, 140),
            (400, 160),
            "max_columns",
        ),
    ],
)
def test_exact_dimension_overflow_is_rejected_per_component(
    xs, ys, size, expected_limit
):
    with grid_image(width=size[0], height=size[1], xs=xs, ys=ys) as image:
        result = detect_ruled_table(image, balanced_config())
    assert result.status == "invalid_grid"
    assert result.diagnostics["limit"] == expected_limit
    assert result.diagnostics["cells_allocated"] == 0


def test_exact_fifty_by_thirty_grid_is_accepted_with_bounded_graph():
    xs = _regular_coordinates(31)
    ys = _regular_coordinates(51)
    with grid_image(width=380, height=630, xs=xs, ys=ys) as image:
        result = detect_ruled_table(image, balanced_config())
    assert result.status == "detected"
    assert result.grid is not None
    assert (result.grid.rows, result.grid.columns) == (50, 30)
    assert len(result.grid.cells) == 1_500
    assert result.diagnostics["intersection_checks"] == 51 * 31
    assert result.diagnostics["component_node_visits"] == 51 + 31
    assert result.diagnostics["component_edge_visits"] == 2 * 51 * 31


def test_two_separate_valid_twenty_five_row_grids_are_unsupported():
    image = Image.new("L", (390, 720), 255)
    draw = ImageDraw.Draw(image)
    for xs, ys in (
        ((10, 80, 150), _regular_coordinates(26)),
        ((220, 290, 360), _regular_coordinates(26, start=400)),
    ):
        for x in xs:
            draw.line((x, ys[0], x, ys[-1]), fill=0, width=2)
        for y in ys:
            draw.line((xs[0], y, xs[-1], y), fill=0, width=2)
    try:
        result = detect_ruled_table(image, balanced_config())
    finally:
        image.close()
    assert result.status == "unsupported"
    assert result.diagnostics["complete_regions"] == 2


def test_dense_structured_global_budget_has_operation_count_proof():
    xs = _regular_coordinates(62)
    ys = _regular_coordinates(102)
    with grid_image(width=752, height=1_230, xs=xs, ys=ys) as image:
        result = detect_ruled_table(image, balanced_config())
    assert result.status == "invalid_grid"
    assert result.diagnostics["intersection_budget"] == 102 * 62
    assert result.diagnostics["intersection_checks"] == 102 * 62
    assert result.diagnostics["intersection_edges"] == 102 * 62
    assert result.diagnostics["component_node_visits"] == 102 + 62
    assert result.diagnostics["component_edge_visits"] == 2 * 102 * 62


def test_excessive_global_lines_are_rejected_before_adjacency_allocation():
    xs = (20, 120, 220)
    ys = _regular_coordinates(103)
    with grid_image(width=240, height=1_250, xs=xs, ys=ys) as image:
        result = detect_ruled_table(image, balanced_config())
    assert result.status == "invalid_grid"
    assert result.diagnostics["limit"] == "global_horizontal_lines"
    assert result.diagnostics["intersection_checks"] == 0
    assert result.diagnostics["adjacency_nodes_allocated"] == 0


def test_grid_rejects_1501_cells_before_coordinate_validation():
    box = Box(0, 0, 1, 1)
    malformed = GridCell(0, 0, box, box)
    with pytest.raises(ValueError, match="max_cells"):
        Grid(
            rows=50,
            columns=30,
            working_table_box=box,
            original_table_box=box,
            cells=(malformed,) * 1_501,
        )


@pytest.mark.parametrize("size", [(0, 0), (0, 10), (10, 0)])
def test_zero_sized_images_return_typed_invalid_without_exception(size):
    with Image.new("L", size, 255) as image:
        result = detect_ruled_table(image, balanced_config())
    assert result.status == "invalid_grid"
    assert result.diagnostics["limit"] == "positive_dimensions"


@pytest.mark.parametrize(
    ("size", "expected_limit"),
    [((10_001, 1), "max_dimension"), ((10_000, 5_001), "max_pixels")],
)
def test_image_bounds_are_checked_before_pixel_access(size, expected_limit):
    image = Image.new("L", (1, 1), 255)
    image._size = size
    try:
        result = detect_ruled_table(image, balanced_config())
    finally:
        image._size = (1, 1)
        image.close()
    assert result.status == "invalid_grid"
    assert result.diagnostics["limit"] == expected_limit
    assert result.diagnostics["cells_allocated"] == 0


def test_geometry_contracts_are_frozen_and_validate_complete_row_major_grid():
    with pytest.raises(ValueError, match="positive area"):
        Box(0, 0, 0, 1)
    box = Box(0, 0, 10, 10)
    cell = GridCell(0, 0, box, box)
    grid = Grid(1, 1, box, box, (cell,))
    with pytest.raises(FrozenInstanceError):
        cell.row = 2
    with pytest.raises(ValueError, match="row-major"):
        Grid(1, 2, box, box, (GridCell(0, 1, box, box), cell))
    with pytest.raises(ValueError, match="rectangular topology"):
        Grid(
            1,
            2,
            box,
            box,
            (cell, GridCell(0, 1, box, box)),
        )
    assert grid.cells == (cell,)


def test_detector_config_rejects_unbounded_or_unsorted_settings():
    with pytest.raises(ValueError, match="max_pixels"):
        balanced_config(max_pixels=50_000_001)
    with pytest.raises(ValueError, match="deskew"):
        balanced_config(deskew_angles_degrees=(0.0, -1.5))
    with pytest.raises(ValueError, match="max_rows.*max_columns.*max_cells"):
        balanced_config(max_cells=100)


def test_ruled_table_config_integration_produces_typed_canonical_candidate():
    from experiments.ruled_table import load_config

    config = detector_config(load_config(CONFIG), "balanced-psm6")
    assert isinstance(config, DetectorConfig)
    assert config.dark_max == 160
    assert config.max_rows == 50
    assert config.max_pixels == 50_000_000


def test_prepare_working_image_matches_result_without_owning_input():
    image = grid_image()
    result = detect_ruled_table(image, balanced_config())
    assert not any(
        isinstance(getattr(result, item.name), Image.Image) for item in fields(result)
    )
    working = prepare_working_image(image, result.deskew_angle_degrees)
    try:
        assert working is not image
        assert working.size == result.working_size
        working.close()
        assert image.getpixel((0, 0)) == 255
    finally:
        image.close()


def test_diagnostics_contain_only_scalar_counts_and_geometry():
    with grid_image() as image:
        result = detect_ruled_table(image, balanced_config())
    assert result.diagnostics
    assert all(isinstance(key, str) for key in result.diagnostics)
    assert all(isinstance(value, (int, float, str)) for value in result.diagnostics.values())
    assert not any("text" in key or "content" in key for key in result.diagnostics)


def test_100_seed_fuzz_returns_bounded_typed_result():
    statuses = {"detected", "not_detected", "unsupported", "invalid_grid"}
    for seed in range(100):
        rng = random.Random(seed)
        size = (rng.randint(1, 256), rng.randint(1, 256))
        payload = rng.randbytes(size[0] * size[1])
        with Image.frombytes("L", size, payload) as image:
            result = detect_ruled_table(image, balanced_config())
        assert result.status in statuses
        assert result.working_size == size
        if result.grid is not None:
            for cell in result.grid.cells:
                for box in (cell.working_box, cell.original_box):
                    assert 0 <= box.left < box.right <= size[0]
                    assert 0 <= box.top < box.bottom <= size[1]
