from __future__ import annotations

import sys
from pathlib import Path

import pymupdf
import pytest
from fastapi.testclient import TestClient
from PIL import Image

sys.path.insert(0, str(Path(__file__).parents[1]))

from markhand_ocr.api import create_app  # noqa: E402
from markhand_ocr.models import OcrSpan  # noqa: E402
from markhand_ocr.service import ConvertRequest  # noqa: E402


def make_pdf(*, pages: int = 1) -> bytes:
    document = pymupdf.open()
    try:
        for page_number in range(1, pages + 1):
            page = document.new_page(width=100, height=50)
            page.insert_text((5, 15), f"sensitive page {page_number}")
        return document.tobytes()
    finally:
        document.close()


class FakeBackend:
    name = "fake"

    def __init__(self, failure: Exception | None = None) -> None:
        self.failure = failure

    def recognize(self, image: Image.Image) -> list[OcrSpan]:
        if self.failure is not None:
            raise self.failure
        return [
            OcrSpan(
                text="recognized",
                confidence=0.9,
                polygon=((1, 1), (20, 1), (20, 10), (1, 10)),
            )
        ]


@pytest.fixture
def client() -> TestClient:
    return TestClient(create_app(FakeBackend()))


def test_health_reports_ready_without_model_identity(client: TestClient) -> None:
    response = client.get("/healthz")

    assert response.status_code == 200
    assert response.json() == {"status": "ready", "backend": "fake"}


def test_convert_rejects_non_pdf(client: TestClient) -> None:
    response = client.post(
        "/v1/convert",
        files={"file": ("x.txt", b"not pdf", "text/plain")},
    )

    assert response.status_code == 415


def test_convert_rejects_pdf_mime_with_invalid_pdf(client: TestClient) -> None:
    response = client.post(
        "/v1/convert",
        files={"file": ("x.pdf", b"%PDF-1.7\nsensitive invalid bytes", "application/pdf")},
    )

    assert response.status_code == 400
    assert response.json() == {"detail": "invalid PDF"}
    assert "sensitive" not in response.text


def test_convert_selects_requested_page(client: TestClient) -> None:
    response = client.post(
        "/v1/convert",
        files={"file": ("two.pdf", make_pdf(pages=2), "application/pdf")},
        data={"pages": "2"},
    )

    assert response.status_code == 200
    payload = response.json()
    assert [page["page_number"] for page in payload["pages"]] == [2]
    assert payload["backend"] == "fake"
    assert payload["markdown"].startswith("<!-- Trang 2 (fake) -->")


def test_convert_maps_bounds_to_payload_or_validation_status() -> None:
    payload_client = TestClient(
        create_app(FakeBackend(), limits=ConvertRequest(max_input_bytes=8))
    )
    payload_response = payload_client.post(
        "/v1/convert",
        files={"file": ("x.pdf", make_pdf(), "application/pdf")},
    )
    assert payload_response.status_code == 413

    selection_client = TestClient(create_app(FakeBackend()))
    selection_response = selection_client.post(
        "/v1/convert",
        files={"file": ("x.pdf", make_pdf(), "application/pdf")},
        data={"pages": "2"},
    )
    assert selection_response.status_code == 422


def test_convert_sanitizes_unexpected_backend_failure() -> None:
    client = TestClient(
        create_app(FakeBackend(RuntimeError("secret recognized document text")))
    )

    response = client.post(
        "/v1/convert",
        files={"file": ("x.pdf", make_pdf(), "application/pdf")},
    )

    assert response.status_code == 502
    assert response.json() == {"detail": "OCR backend failed"}
    assert "secret" not in response.text
    assert "document text" not in response.text
