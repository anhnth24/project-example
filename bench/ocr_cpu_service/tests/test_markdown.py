from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parents[1]))

from markhand_ocr.markdown import spans_to_markdown  # noqa: E402
from markhand_ocr.models import OcrSpan, PageResult  # noqa: E402


def page(texts: list[str], page_number: int) -> PageResult:
    spans = tuple(
        OcrSpan(
            text=text,
            confidence=0.9,
            polygon=((10, y), (200, y), (200, y + 20), (10, y + 20)),
        )
        for y, text in enumerate(texts, start=1)
    )
    return PageResult(
        page_number=page_number,
        width=1000,
        height=1400,
        spans=spans,
        backend="PP-OCRv6",
    )


def test_markdown_normalizes_nfc_and_preserves_page_boundary() -> None:
    pages = [page(["Co\u0323\u0302ng ho\u0300a"], 1), page(["Trang hai"], 2)]

    assert spans_to_markdown(pages) == (
        "<!-- Trang 1 (PP-OCRv6) -->\n\nCộng hòa\n\n"
        "<!-- Trang 2 (PP-OCRv6) -->\n\nTrang hai\n"
    )
