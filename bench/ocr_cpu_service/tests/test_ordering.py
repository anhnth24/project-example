from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parents[1]))

from markhand_ocr.models import OcrSpan  # noqa: E402
from markhand_ocr.ordering import order_spans  # noqa: E402


def span(text: str, x: int, y: int) -> OcrSpan:
    return OcrSpan(
        text=text,
        confidence=0.9,
        polygon=((x, y), (x + 120, y), (x + 120, y + 30), (x, y + 30)),
    )


def wide_span(text: str, y: int) -> OcrSpan:
    return OcrSpan(
        text=text,
        confidence=0.9,
        polygon=((50, y), (950, y), (950, y + 40), (50, y + 40)),
    )


def column_span(text: str, x: int, y: int) -> OcrSpan:
    return OcrSpan(
        text=text,
        confidence=0.9,
        polygon=((x, y), (x + 400, y), (x + 400, y + 30), (x, y + 30)),
    )


def texts(spans: list[OcrSpan]) -> list[str]:
    return [item.text for item in spans]


def test_orders_two_columns_top_to_bottom_then_left_to_right() -> None:
    spans = [
        span("R2", 600, 200),
        span("L2", 100, 200),
        span("R1", 600, 100),
        span("L1", 100, 100),
    ]

    assert texts(order_spans(spans, page_width=1000)) == ["L1", "L2", "R1", "R2"]


def test_full_width_heading_precedes_columns() -> None:
    spans = [wide_span("TITLE", 20), span("L", 100, 100), span("R", 600, 100)]

    assert texts(order_spans(spans, page_width=1000)) == ["TITLE", "L", "R"]


def test_orders_y_overlapping_spans_left_to_right_within_a_line() -> None:
    spans = [span("A-right", 300, 105), span("Z-left", 100, 100)]

    assert texts(order_spans(spans, page_width=1000)) == ["Z-left", "A-right"]


def test_separated_wide_column_spans_are_not_a_full_width_block() -> None:
    spans = [
        column_span("R2", 550, 200),
        column_span("L2", 50, 200),
        column_span("R1", 550, 100),
        column_span("L1", 50, 100),
    ]

    assert texts(order_spans(spans, page_width=1000)) == ["L1", "L2", "R1", "R2"]
