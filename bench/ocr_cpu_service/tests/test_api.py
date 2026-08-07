from __future__ import annotations

import asyncio
import io
import inspect
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import Any

import pypdfium2 as pdfium
import pytest
from fastapi.testclient import TestClient
from PIL import Image

sys.path.insert(0, str(Path(__file__).parents[1]))

from markhand_ocr.api import ConversionAdmission, create_app  # noqa: E402
from markhand_ocr.models import OcrSpan  # noqa: E402
from markhand_ocr.service import ConvertRequest, InvalidPdf  # noqa: E402


def make_pdf(*, pages: int = 1) -> bytes:
    document = pdfium.PdfDocument.new()
    created_pages: list[pdfium.PdfPage] = []
    output = io.BytesIO()
    try:
        for _ in range(pages):
            created_pages.append(document.new_page(100, 50))
        document.save(output)
        return output.getvalue()
    finally:
        for page in created_pages:
            page.close()
        document.close()


class FakeBackend:
    name = "fake"

    def __init__(self, failure: Exception | None = None) -> None:
        self.failure = failure
        self.calls = 0

    def recognize(self, image: Image.Image) -> list[OcrSpan]:
        self.calls += 1
        if self.failure is not None:
            raise self.failure
        return [
            OcrSpan(
                text="recognized",
                confidence=0.9,
                polygon=((1, 1), (20, 1), (20, 10), (1, 10)),
            )
        ]


class BlockingBackend(FakeBackend):
    def __init__(self) -> None:
        super().__init__()
        self.started = threading.Event()
        self.release = threading.Event()

    def recognize(self, image: Image.Image) -> list[OcrSpan]:
        self.started.set()
        if not self.release.wait(timeout=5):
            raise RuntimeError("test backend was not released")
        return super().recognize(image)


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


def test_backend_exception_type_cannot_be_misclassified_as_pdf_failure() -> None:
    client = TestClient(
        create_app(FakeBackend(InvalidPdf("secret backend document text")))
    )

    response = client.post(
        "/v1/convert",
        files={"file": ("x.pdf", make_pdf(), "application/pdf")},
    )

    assert response.status_code == 502
    assert response.json() == {"detail": "OCR backend failed"}
    assert "secret" not in response.text


def call_asgi(
    app: Any, body: bytes, *, content_length: bytes | None
) -> tuple[int, bytes]:
    headers = [
        (b"content-type", b"multipart/form-data; boundary=review-boundary"),
    ]
    if content_length is not None:
        headers.append((b"content-length", content_length))
    scope = {
        "type": "http",
        "asgi": {"version": "3.0"},
        "http_version": "1.1",
        "method": "POST",
        "scheme": "http",
        "path": "/v1/convert",
        "raw_path": b"/v1/convert",
        "query_string": b"",
        "headers": headers,
        "client": ("127.0.0.1", 1),
        "server": ("test", 80),
    }
    sent: list[dict[str, Any]] = []
    delivered = False

    async def receive() -> dict[str, Any]:
        nonlocal delivered
        if delivered:
            return {"type": "http.disconnect"}
        delivered = True
        return {"type": "http.request", "body": body, "more_body": False}

    async def send(message: dict[str, Any]) -> None:
        sent.append(message)

    asyncio.run(app(scope, receive, send))
    start = next(message for message in sent if message["type"] == "http.response.start")
    response_body = b"".join(
        message.get("body", b"")
        for message in sent
        if message["type"] == "http.response.body"
    )
    return start["status"], response_body


@pytest.mark.parametrize("content_length", [None, b"1"])
def test_body_limit_rejects_oversized_stream_without_trusting_content_length(
    content_length: bytes | None,
) -> None:
    backend = FakeBackend()
    app = create_app(backend, max_body_bytes=64)
    oversized_multipart = b"--review-boundary\r\n" + b"x" * 128

    status, body = call_asgi(
        app, oversized_multipart, content_length=content_length
    )

    assert status == 413
    assert body == b'{"detail":"request body too large"}'
    assert backend.calls == 0


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("pages", "secret-page-value"),
        ("dpi", "secret-dpi-value"),
    ],
)
def test_validation_errors_are_sanitized(
    client: TestClient, field: str, value: str
) -> None:
    response = client.post(
        "/v1/convert",
        files={"file": ("x.pdf", make_pdf(), "application/pdf")},
        data={field: value},
    )

    assert response.status_code == 400
    assert response.json() == {"detail": "invalid request"}
    assert value not in response.text


def test_pdf_render_failure_is_sanitized_as_invalid_pdf(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import markhand_ocr.service as service

    def fail_render(page: object, limits: object) -> Image.Image:
        raise RuntimeError("secret PDF renderer detail")

    monkeypatch.setattr(service, "render_page", fail_render)
    client = TestClient(create_app(FakeBackend()))

    response = client.post(
        "/v1/convert",
        files={"file": ("x.pdf", make_pdf(), "application/pdf")},
    )

    assert response.status_code == 400
    assert response.json() == {"detail": "invalid PDF"}
    assert "secret" not in response.text


def test_conversion_endpoint_is_synchronous() -> None:
    app = create_app(FakeBackend())
    endpoint = next(
        route.endpoint
        for route in app.routes
        if getattr(route, "path", None) == "/v1/convert"
    )

    assert inspect.iscoroutinefunction(endpoint) is False


def test_conversion_deadline_retains_capacity_until_underlying_work_finishes() -> None:
    backend = BlockingBackend()
    admission = ConversionAdmission()
    app = create_app(
        backend,
        admission=admission,
        acquisition_timeout_seconds=0.03,
        conversion_deadline_seconds=0.05,
    )

    with TestClient(app) as client:
        timed_out = client.post(
            "/v1/convert",
            files={"file": ("first.pdf", make_pdf(), "application/pdf")},
        )
        assert backend.started.is_set()
        assert timed_out.status_code == 504
        assert timed_out.json() == {"detail": "conversion deadline exceeded"}

        saturated_started = time.monotonic()
        saturated = client.post(
            "/v1/convert",
            files={"file": ("second.pdf", make_pdf(), "application/pdf")},
        )
        saturated_elapsed = time.monotonic() - saturated_started
        assert saturated.status_code == 503
        assert saturated.json() == {"detail": "conversion capacity unavailable"}
        assert saturated_elapsed < 0.5
        assert backend.calls == 0

        backend.release.set()
        with ThreadPoolExecutor(max_workers=1) as executor:
            eventual = executor.submit(
                client.post,
                "/v1/convert",
                files={
                    "file": ("eventual.pdf", make_pdf(), "application/pdf")
                },
            )
            response = eventual.result(timeout=2)

        assert response.status_code == 200
        assert backend.calls == 2
