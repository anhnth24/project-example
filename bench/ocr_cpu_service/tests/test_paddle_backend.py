from __future__ import annotations

import os
import sys
import unicodedata
from pathlib import Path
from typing import Any

import pytest
from fastapi.testclient import TestClient
from PIL import Image, ImageDraw, ImageFont

sys.path.insert(0, str(Path(__file__).parents[1]))

from markhand_ocr.paddle_backend import (  # noqa: E402
    PaddleOcrBackend,
    adapt_result,
    create_runtime_app,
)
from markhand_ocr.models import OcrSpan  # noqa: E402


class InjectedBackend:
    name = "fake"

    def recognize(self, image: Image.Image) -> list[OcrSpan]:
        del image
        return []


def test_adapts_paddle_result_without_numpy_values() -> None:
    spans = adapt_result(
        {
            "dt_polys": [[[1, 2], [5, 2], [5, 8], [1, 8]]],
            "rec_texts": ["Cộng hòa"],
            "rec_scores": [0.98],
        }
    )

    assert spans[0].text == "Cộng hòa"
    assert spans[0].confidence == pytest.approx(0.98)
    assert spans[0].polygon == ((1, 2), (5, 2), (5, 8), (1, 8))
    assert isinstance(spans[0].confidence, float)
    assert all(
        isinstance(coordinate, (int, float))
        for point in spans[0].polygon
        for coordinate in point
    )


@pytest.mark.parametrize(
    "result",
    [
        {
            "dt_polys": [],
            "rec_texts": ["orphan"],
            "rec_scores": [],
        },
        {
            "dt_polys": [[[1, 2], [5, 2], [5, 8], [1, 8]]],
            "rec_texts": [],
            "rec_scores": [0.98],
        },
    ],
)
def test_rejects_mismatched_documented_result_lengths(
    result: dict[str, list[Any]],
) -> None:
    with pytest.raises(ValueError, match="length"):
        adapt_result(result)


def test_backend_initializes_cpu_pipeline_once_and_adapts_predict_result() -> None:
    calls: list[dict[str, object]] = []

    class FakeResult(dict[str, object]):
        pass

    class FakePipeline:
        def __init__(self) -> None:
            self.images: list[object] = []

        def predict(self, image: object) -> list[FakeResult]:
            self.images.append(image)
            return [
                FakeResult(
                    dt_polys=[[[1, 2], [5, 2], [5, 8], [1, 8]]],
                    rec_texts=["Cộng hòa"],
                    rec_scores=[0.98],
                )
            ]

    pipeline = FakePipeline()

    def factory(**kwargs: object) -> FakePipeline:
        calls.append(kwargs)
        return pipeline

    backend = PaddleOcrBackend(pipeline_factory=factory)
    image = Image.new("RGB", (10, 10), "white")
    try:
        first = backend.recognize(image)
        second = backend.recognize(image)
    finally:
        image.close()

    assert calls == [
        {
            "device": "cpu",
            "use_doc_orientation_classify": False,
            "use_doc_unwarping": False,
            "use_textline_orientation": False,
        }
    ]
    assert len(pipeline.images) == 2
    assert getattr(pipeline.images[0], "shape", None) == (10, 10, 3)
    assert first == second
    assert first[0].text == "Cộng hòa"
    assert backend.name == "paddle"


def test_runtime_selection_preserves_backend_injection_and_safe_health(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MARKHAND_OCR_BACKEND", "paddle")
    backend = InjectedBackend()

    response = TestClient(
        create_runtime_app(backend_factory=lambda: backend)
    ).get("/healthz")

    assert response.status_code == 200
    assert response.json() == {"status": "ready", "backend": "fake"}


def test_runtime_rejects_unselected_backend(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("MARKHAND_OCR_BACKEND", raising=False)

    with pytest.raises(ValueError, match="MARKHAND_OCR_BACKEND"):
        create_runtime_app()


@pytest.mark.live
@pytest.mark.skipif(
    os.environ.get("MARKHAND_OCR_LIVE") != "1",
    reason="set MARKHAND_OCR_LIVE=1 to run the cached-model smoke",
)
def test_live_generated_vietnamese_page_returns_nfc_text_on_cpu() -> None:
    image = Image.new("RGB", (1400, 260), "white")
    draw = ImageDraw.Draw(image)
    font = ImageFont.truetype(
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", 72
    )
    draw.text((40, 70), "Cộng hòa Việt Nam", fill="black", font=font)

    try:
        spans = PaddleOcrBackend().recognize(image)
    finally:
        image.close()

    text = " ".join(span.text for span in spans).strip()
    assert text
    assert unicodedata.is_normalized("NFC", text)
