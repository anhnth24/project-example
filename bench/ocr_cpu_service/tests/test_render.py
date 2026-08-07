from __future__ import annotations

import io
import sys
from pathlib import Path

import pypdfium2 as pdfium

sys.path.insert(0, str(Path(__file__).parents[1]))

import benchmark.run as benchmark_run  # noqa: E402
from benchmark.run import generate_reviewed_multicolumn_case  # noqa: E402
from markhand_ocr.render import RenderLimits, open_pdf, render_page  # noqa: E402


def make_pdf(*, width: float = 100, height: float = 50) -> bytes:
    document = pdfium.PdfDocument.new()
    page = document.new_page(width, height)
    output = io.BytesIO()
    try:
        document.save(output)
        return output.getvalue()
    finally:
        page.close()
        document.close()


def test_opens_and_renders_with_pdfium() -> None:
    with open_pdf(make_pdf()) as document:
        assert isinstance(document, pdfium.PdfDocument)
        assert len(document) == 1
        page = document[0]
        try:
            image = render_page(
                page,
                RenderLimits(dpi=144, max_pixels=1_000_000, max_dimension=1_000),
            )
        finally:
            page.close()

    try:
        assert image.mode == "RGB"
        assert image.size == (200, 100)
    finally:
        image.close()


def test_benchmark_pdf_inspection_uses_the_same_pdfium_runtime() -> None:
    assert benchmark_run.pdfium is pdfium
    assert not hasattr(benchmark_run, "pymupdf")


def test_generates_deterministic_source_ground_truth_multicolumn_case(
    tmp_path: Path,
) -> None:
    first_metadata, first_page = generate_reviewed_multicolumn_case(tmp_path)
    first_bytes = first_page.path.read_bytes()
    second_metadata, second_page = generate_reviewed_multicolumn_case(tmp_path)

    assert second_page.path.read_bytes() == first_bytes
    assert first_metadata == second_metadata
    assert first_page.source_id == "reviewed-multicolumn-v1"
    assert first_page.stratum == "reviewed-multicolumn"
    assert first_page.gate_included is False
    assert first_page.reference is None
    assert first_page.reading_order_anchors == (
        "L1",
        "L2",
        "L3",
        "R1",
        "R2",
        "R3",
    )
    assert first_metadata["ground_truth"] == "deterministic-source"
    assert "text" not in first_metadata
