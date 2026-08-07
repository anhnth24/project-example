"""Bounded in-memory PDF opening and page rendering."""

from __future__ import annotations

import math
from contextlib import contextmanager
from dataclasses import dataclass
from typing import Iterator

import pymupdf
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
def open_pdf(data: bytes) -> Iterator[pymupdf.Document]:
    """Open PDF bytes and guarantee native document cleanup."""
    document: pymupdf.Document | None = None
    try:
        try:
            document = pymupdf.open(stream=data, filetype="pdf")
        except (pymupdf.FileDataError, RuntimeError, ValueError) as error:
            raise PdfOpenError("invalid PDF") from error

        if not document.is_pdf:
            raise PdfOpenError("invalid PDF")
        if document.needs_pass:
            raise PdfOpenError("encrypted PDF")
        yield document
    finally:
        if document is not None:
            document.close()


def render_page(page: pymupdf.Page, limits: RenderLimits) -> Image.Image:
    """Render one page after validating dimensions without image allocation."""
    scale = limits.dpi / 72
    width = math.ceil(page.rect.width * scale)
    height = math.ceil(page.rect.height * scale)
    if width <= 0 or height <= 0:
        raise PageRenderRejected("invalid page dimensions")
    if width > limits.max_dimension or height > limits.max_dimension:
        raise PageRenderRejected("dimension limit exceeded")
    if width * height > limits.max_pixels:
        raise PageRenderRejected("pixel limit exceeded")

    pixmap = page.get_pixmap(
        matrix=pymupdf.Matrix(scale, scale),
        colorspace=pymupdf.csRGB,
        alpha=False,
    )
    return Image.frombytes("RGB", (pixmap.width, pixmap.height), pixmap.samples)

