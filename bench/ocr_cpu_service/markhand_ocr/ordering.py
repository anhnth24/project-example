"""Geometry-only reading order for basic one- and two-column pages."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Sequence

from .models import OcrSpan

_FULL_WIDTH_COVERAGE = 0.6
_MIN_GUTTER_WIDTH = 0.08


@dataclass(frozen=True, slots=True)
class _BoxedSpan:
    span: OcrSpan
    index: int
    left: float
    top: float
    right: float
    bottom: float


def order_spans(spans: Sequence[OcrSpan], page_width: int) -> list[OcrSpan]:
    """Order spans using only their page geometry."""
    if page_width <= 0:
        raise ValueError("page_width must be positive")

    boxed = [_boxed_span(item, index) for index, item in enumerate(spans)]
    lines = _group_lines(boxed)
    ordered: list[_BoxedSpan] = []
    region: list[_BoxedSpan] = []

    for line in lines:
        if _horizontal_coverage(line) >= page_width * _FULL_WIDTH_COVERAGE:
            ordered.extend(_order_region(region, page_width))
            ordered.extend(sorted(line, key=_horizontal_key))
            region = []
        else:
            region.extend(line)

    ordered.extend(_order_region(region, page_width))
    return [item.span for item in ordered]


def _boxed_span(span: OcrSpan, index: int) -> _BoxedSpan:
    if not span.polygon:
        raise ValueError("span polygon must not be empty")
    xs = [point[0] for point in span.polygon]
    ys = [point[1] for point in span.polygon]
    return _BoxedSpan(span, index, min(xs), min(ys), max(xs), max(ys))


def _group_lines(spans: Sequence[_BoxedSpan]) -> list[list[_BoxedSpan]]:
    lines: list[list[_BoxedSpan]] = []
    for item in sorted(spans, key=_vertical_key):
        for line in lines:
            if any(_overlaps_vertically(item, member) for member in line):
                line.append(item)
                break
        else:
            lines.append([item])

    for line in lines:
        line.sort(key=_horizontal_key)
    lines.sort(key=lambda line: min(_vertical_key(item) for item in line))
    return lines


def _order_region(spans: Sequence[_BoxedSpan], page_width: int) -> list[_BoxedSpan]:
    if not spans:
        return []

    gutter = _stable_gutter(spans, page_width)
    if gutter is None:
        return _order_top_to_bottom(spans)

    left = [item for item in spans if (item.left + item.right) / 2 < gutter]
    right = [item for item in spans if (item.left + item.right) / 2 >= gutter]
    if not left or not right:
        return _order_top_to_bottom(spans)
    return _order_top_to_bottom(left) + _order_top_to_bottom(right)


def _stable_gutter(spans: Sequence[_BoxedSpan], page_width: int) -> float | None:
    intervals = sorted((item.left, item.right) for item in spans)
    candidates: list[tuple[float, float]] = []
    furthest_right = intervals[0][1]

    for left, right in intervals[1:]:
        if left > furthest_right:
            candidates.append((left - furthest_right, (left + furthest_right) / 2))
        furthest_right = max(furthest_right, right)

    if not candidates:
        return None
    width, center = max(candidates, key=lambda candidate: (candidate[0], -candidate[1]))
    if width < page_width * _MIN_GUTTER_WIDTH:
        return None
    return center


def _order_top_to_bottom(spans: Sequence[_BoxedSpan]) -> list[_BoxedSpan]:
    return [item for line in _group_lines(spans) for item in line]


def _horizontal_coverage(spans: Sequence[_BoxedSpan]) -> float:
    intervals = sorted((item.left, item.right) for item in spans)
    coverage = 0.0
    start, end = intervals[0]
    for left, right in intervals[1:]:
        if left <= end:
            end = max(end, right)
        else:
            coverage += end - start
            start, end = left, right
    return coverage + end - start


def _overlaps_vertically(left: _BoxedSpan, right: _BoxedSpan) -> bool:
    return min(left.bottom, right.bottom) >= max(left.top, right.top)


def _vertical_key(item: _BoxedSpan) -> tuple[float, float, float, float, int]:
    return item.top, item.left, item.bottom, item.right, item.index


def _horizontal_key(item: _BoxedSpan) -> tuple[float, float, float, float, int]:
    return item.left, item.top, item.right, item.bottom, item.index
