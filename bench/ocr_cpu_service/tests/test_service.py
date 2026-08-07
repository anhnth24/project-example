from __future__ import annotations

import io
import sys
from contextlib import contextmanager
from pathlib import Path
from typing import Iterator

import pypdfium2 as pdfium
import pytest
from PIL import Image

sys.path.insert(0, str(Path(__file__).parents[1]))

from markhand_ocr.models import OcrSpan  # noqa: E402
from markhand_ocr.service import (  # noqa: E402
    BackendFailure,
    ConversionRejected,
    ConvertRequest,
    InvalidPdf,
    convert_pdf,
)


def make_pdf(*, pages: int = 1, width: float = 200, height: float = 100) -> bytes:
    document = pdfium.PdfDocument.new()
    created_pages: list[pdfium.PdfPage] = []
    output = io.BytesIO()
    try:
        for _ in range(pages):
            created_pages.append(document.new_page(width, height))
        document.save(output)
        return output.getvalue()
    finally:
        for page in created_pages:
            page.close()
        document.close()


@pytest.fixture
def tiny_pdf() -> bytes:
    return make_pdf()


@pytest.fixture
def two_page_pdf() -> bytes:
    return make_pdf(pages=2)


class FakeBackend:
    name = "fake"

    def __init__(self, *, failure: Exception | None = None) -> None:
        self.failure = failure
        self.images: list[Image.Image] = []

    def recognize(self, image: Image.Image) -> list[OcrSpan]:
        self.images.append(image)
        if self.failure is not None:
            raise self.failure
        width, _ = image.size
        return [
            OcrSpan(
                text="right",
                confidence=0.8,
                polygon=((width * 0.6, 10), (width - 5, 10), (width - 5, 20)),
            ),
            OcrSpan(
                text="left",
                confidence=0.9,
                polygon=((5, 10), (width * 0.4, 10), (width * 0.4, 20)),
            ),
        ]


@pytest.fixture
def fake_backend() -> FakeBackend:
    return FakeBackend()


def test_rejects_page_count_over_limit(
    tiny_pdf: bytes, fake_backend: FakeBackend
) -> None:
    with pytest.raises(ConversionRejected, match="page limit"):
        convert_pdf(tiny_pdf, ConvertRequest(max_pages=0), fake_backend)

    assert fake_backend.images == []


def test_converts_selected_pages_and_reports_diagnostics(
    two_page_pdf: bytes, fake_backend: FakeBackend
) -> None:
    result = convert_pdf(two_page_pdf, ConvertRequest(pages=[2]), fake_backend)

    assert [page.page_number for page in result.pages] == [2]
    assert [span.text for span in result.pages[0].spans] == ["left", "right"]
    assert result.backend == "fake"
    assert result.markdown.startswith("<!-- Trang 2 (fake) -->")
    assert result.duration_ms >= 0
    assert result.peak_rss_bytes > 0


def test_rejects_invalid_page_selection_before_rendering(
    tiny_pdf: bytes, fake_backend: FakeBackend
) -> None:
    with pytest.raises(ConversionRejected, match="page selection"):
        convert_pdf(tiny_pdf, ConvertRequest(pages=[0, 2]), fake_backend)

    assert fake_backend.images == []


def test_rejects_oversized_page_before_pixmap_allocation(
    fake_backend: FakeBackend, monkeypatch: pytest.MonkeyPatch
) -> None:
    allocation_attempted = False
    original_render = pdfium.PdfPage.render

    def tracked_render(
        self: pdfium.PdfPage, *args: object, **kwargs: object
    ) -> object:
        nonlocal allocation_attempted
        allocation_attempted = True
        return original_render(self, *args, **kwargs)

    monkeypatch.setattr(pdfium.PdfPage, "render", tracked_render)

    with pytest.raises(ConversionRejected, match="pixel limit"):
        convert_pdf(
            make_pdf(width=2_000, height=2_000),
            ConvertRequest(dpi=300, max_pixels=1_000),
            fake_backend,
        )

    assert allocation_attempted is False
    assert fake_backend.images == []


def test_rejects_oversized_input_before_opening_pdf(
    tiny_pdf: bytes, fake_backend: FakeBackend, monkeypatch: pytest.MonkeyPatch
) -> None:
    opened = False
    original_open = pdfium.PdfDocument

    def tracked_open(*args: object, **kwargs: object) -> pdfium.PdfDocument:
        nonlocal opened
        opened = True
        return original_open(*args, **kwargs)

    monkeypatch.setattr(pdfium, "PdfDocument", tracked_open)

    with pytest.raises(ConversionRejected, match="input size limit"):
        convert_pdf(
            tiny_pdf,
            ConvertRequest(max_input_bytes=len(tiny_pdf) - 1),
            fake_backend,
        )

    assert opened is False


def test_rejects_malformed_and_encrypted_pdfs(
    fake_backend: FakeBackend, monkeypatch: pytest.MonkeyPatch
) -> None:
    with pytest.raises(InvalidPdf, match="invalid PDF"):
        convert_pdf(b"%PDF-1.7\nnot really a pdf", ConvertRequest(), fake_backend)

    import markhand_ocr.render as render

    original_document = render.pdfium.PdfDocument

    def open_document(data: bytes) -> pdfium.PdfDocument:
        if data == b"%PDF-encrypted":
            raise pdfium.PdfiumError(
                "Failed to load document (PDFium: Incorrect password error)."
            )
        return original_document(data)

    monkeypatch.setattr(render.pdfium, "PdfDocument", open_document)

    with pytest.raises(InvalidPdf, match="encrypted PDF"):
        convert_pdf(b"%PDF-encrypted", ConvertRequest(), fake_backend)


def test_closes_page_image_when_backend_fails(tiny_pdf: bytes) -> None:
    backend = FakeBackend(failure=RuntimeError("secret document text"))

    with pytest.raises(BackendFailure, match="OCR backend failed"):
        convert_pdf(tiny_pdf, ConvertRequest(), backend)

    assert len(backend.images) == 1
    with pytest.raises(ValueError, match="closed"):
        backend.images[0].getpixel((0, 0))


@pytest.mark.parametrize("failure_point", ["load", "geometry", "render"])
def test_maps_pdf_page_failures_to_invalid_pdf(
    tiny_pdf: bytes,
    fake_backend: FakeBackend,
    monkeypatch: pytest.MonkeyPatch,
    failure_point: str,
) -> None:
    import markhand_ocr.service as service

    class BrokenPage:
        def get_size(self) -> tuple[int, int]:
            if failure_point == "geometry":
                raise RuntimeError("secret geometry details")
            return (10, 10)

        def close(self) -> None:
            pass

    class BrokenDocument:
        def __len__(self) -> int:
            return 1

        def __getitem__(self, index: int) -> object:
            assert index == 0
            if failure_point == "load":
                raise RuntimeError("secret page load details")
            return BrokenPage()

    @contextmanager
    def broken_pdf(data: bytes) -> Iterator[BrokenDocument]:
        assert data == tiny_pdf
        yield BrokenDocument()

    monkeypatch.setattr(service, "open_pdf", broken_pdf)
    if failure_point == "render":
        monkeypatch.setattr(
            service,
            "render_page",
            lambda page, limits: (_ for _ in ()).throw(
                RuntimeError("secret renderer details")
            ),
        )

    with pytest.raises(InvalidPdf, match=r"^invalid PDF$"):
        convert_pdf(tiny_pdf, ConvertRequest(), fake_backend)

    assert fake_backend.images == []
