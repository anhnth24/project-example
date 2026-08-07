"""Bounded PDF-to-Markdown orchestration with an injected OCR backend."""

from __future__ import annotations

import resource
import sys
import time
from dataclasses import dataclass
from typing import Sequence

from .backend import OcrBackend
from .markdown import spans_to_markdown
from .models import PageResult
from .ordering import order_spans
from .render import (
    PageRenderRejected,
    PdfOpenError,
    RenderLimits,
    open_pdf,
    render_page,
)


class ConversionRejected(ValueError):
    """The request exceeds an explicit conversion bound."""

    def __init__(self, message: str, *, kind: str) -> None:
        super().__init__(message)
        self.kind = kind


class InvalidPdf(ValueError):
    """The document is malformed, unsupported, or encrypted."""


@dataclass(frozen=True, slots=True)
class ConvertRequest:
    pages: Sequence[int] | None = None
    dpi: int = 144
    max_pages: int = 20
    max_pixels: int = 20_000_000
    max_dimension: int = 10_000
    max_input_bytes: int = 25_000_000


@dataclass(frozen=True, slots=True)
class ConvertResult:
    markdown: str
    pages: tuple[PageResult, ...]
    backend: str
    duration_ms: float
    peak_rss_bytes: int


def convert_pdf(
    data: bytes, request: ConvertRequest, backend: OcrBackend
) -> ConvertResult:
    """Convert local PDF bytes while enforcing bounds before large allocations."""
    started = time.perf_counter_ns()
    _validate_request(request)
    if len(data) > request.max_input_bytes:
        raise ConversionRejected("input size limit exceeded", kind="payload")

    limits = RenderLimits(
        dpi=request.dpi,
        max_pixels=request.max_pixels,
        max_dimension=request.max_dimension,
    )
    pages: list[PageResult] = []
    try:
        with open_pdf(data) as document:
            if document.page_count > request.max_pages:
                raise ConversionRejected("page limit exceeded", kind="page_limit")
            selected_pages = _selected_pages(request.pages, document.page_count)

            for page_number in selected_pages:
                page = document.load_page(page_number - 1)
                try:
                    image = render_page(page, limits)
                except PageRenderRejected as error:
                    raise ConversionRejected(str(error), kind="render_bound") from error

                try:
                    spans = backend.recognize(image)
                    ordered = order_spans(spans, page_width=image.width)
                    pages.append(
                        PageResult(
                            page_number=page_number,
                            width=image.width,
                            height=image.height,
                            spans=tuple(ordered),
                            backend=backend.name,
                        )
                    )
                finally:
                    image.close()
    except PdfOpenError as error:
        raise InvalidPdf(str(error)) from error

    page_results = tuple(pages)
    return ConvertResult(
        markdown=spans_to_markdown(page_results),
        pages=page_results,
        backend=backend.name,
        duration_ms=(time.perf_counter_ns() - started) / 1_000_000,
        peak_rss_bytes=_peak_rss_bytes(),
    )


def _validate_request(request: ConvertRequest) -> None:
    if request.dpi <= 0 or request.dpi > 600:
        raise ConversionRejected("DPI limit exceeded", kind="request")
    if request.max_pages < 0:
        raise ConversionRejected("page limit must not be negative", kind="request")
    if request.max_pixels <= 0 or request.max_dimension <= 0:
        raise ConversionRejected("render limits must be positive", kind="request")
    if request.max_input_bytes < 0:
        raise ConversionRejected("input size limit must not be negative", kind="request")


def _selected_pages(pages: Sequence[int] | None, page_count: int) -> tuple[int, ...]:
    if pages is None:
        return tuple(range(1, page_count + 1))
    selected = tuple(pages)
    if (
        not selected
        or len(selected) != len(set(selected))
        or any(not isinstance(page, int) or isinstance(page, bool) for page in selected)
        or any(page < 1 or page > page_count for page in selected)
    ):
        raise ConversionRejected("invalid page selection", kind="page_selection")
    return selected


def _peak_rss_bytes() -> int:
    peak_rss = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    return int(peak_rss if sys.platform == "darwin" else peak_rss * 1024)

