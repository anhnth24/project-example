"""Serialize ordered OCR page results as NFC Markdown."""

from __future__ import annotations

import unicodedata
from typing import Sequence

from .models import PageResult


def spans_to_markdown(pages: Sequence[PageResult]) -> str:
    page_blocks: list[str] = []
    for page in pages:
        header = f"<!-- Trang {page.page_number} ({page.backend}) -->"
        text = "\n\n".join(span.text for span in page.spans)
        page_blocks.append(f"{header}\n\n{text}" if text else header)

    if not page_blocks:
        return ""
    return unicodedata.normalize("NFC", "\n\n".join(page_blocks) + "\n")
