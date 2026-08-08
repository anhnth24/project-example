"""Bounded PDFium opening and page rendering for benchmark inputs."""

from __future__ import annotations

import math
from contextlib import contextmanager
from dataclasses import dataclass
from typing import Iterator

import pypdfium2 as pdfium
from PIL import Image


class PdfOpenError(ValueError):
    """The supplied bytes are not an open, unencrypted PDF."""


class PageRenderRejected(ValueError):
    """A page cannot be rendered within configured allocation bounds."""


@dataclass(frozen=True, slots=True)
class RenderLimits:
    dpi: int
    max_pixels: int
    max_dimension: int


@contextmanager
def open_pdf(data: bytes) -> Iterator[pdfium.PdfDocument]:
    """Open PDF bytes and guarantee native document cleanup."""
    document: pdfium.PdfDocument | None = None
    try:
        try:
            document = pdfium.PdfDocument(data)
        except (pdfium.PdfiumError, RuntimeError, ValueError) as error:
            message = (
                "encrypted PDF"
                if "password" in str(error).lower()
                else "invalid PDF"
            )
            raise PdfOpenError(message) from error
        yield document
    finally:
        if document is not None:
            document.close()


def render_page(page: pdfium.PdfPage, limits: RenderLimits) -> Image.Image:
    """Render one page after validating dimensions without image allocation."""
    scale = limits.dpi / 72
    page_width, page_height = page.get_size()
    width = math.ceil(page_width * scale)
    height = math.ceil(page_height * scale)
    if width <= 0 or height <= 0:
        raise PageRenderRejected("invalid page dimensions")
    if width > limits.max_dimension or height > limits.max_dimension:
        raise PageRenderRejected("dimension limit exceeded")
    if width * height > limits.max_pixels:
        raise PageRenderRejected("pixel limit exceeded")

    bitmap = page.render(scale=scale, may_draw_forms=True)
    try:
        rendered = bitmap.to_pil()
        try:
            return rendered.convert("RGB").copy()
        finally:
            rendered.close()
    finally:
        bitmap.close()


def bounded_sample_render_limits(
    *,
    page_width: float,
    page_height: float,
    requested_dpi: int,
    max_pixels: int = 20_000_000,
    max_dimension: int = 5_000,
) -> RenderLimits:
    """Choose the highest integer DPI that fits explicit sample image bounds."""
    if page_width <= 0 or page_height <= 0 or requested_dpi <= 0:
        raise ValueError("page dimensions and DPI must be positive")
    scale = min(
        requested_dpi / 72,
        max_dimension / page_width,
        max_dimension / page_height,
        math.sqrt(max_pixels / (page_width * page_height)),
    )
    dpi = max(1, math.floor(scale * 72))
    while (
        math.ceil(page_width * dpi / 72) > max_dimension
        or math.ceil(page_height * dpi / 72) > max_dimension
        or math.ceil(page_width * dpi / 72)
        * math.ceil(page_height * dpi / 72)
        > max_pixels
    ):
        dpi -= 1
        if dpi <= 0:
            raise ValueError("page cannot fit bounded render dimensions")
    return RenderLimits(
        dpi=dpi,
        max_pixels=max_pixels,
        max_dimension=max_dimension,
    )
