from __future__ import annotations

import os
import sys
import threading
import time
import unicodedata
from concurrent.futures import ThreadPoolExecutor
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


def _require_cached_model_dirs() -> tuple[Path, Path]:
    environment_names = (
        "MARKHAND_OCR_LIVE_DETECTION_MODEL_DIR",
        "MARKHAND_OCR_LIVE_RECOGNITION_MODEL_DIR",
    )
    raw_dirs = tuple(os.environ.get(name) for name in environment_names)
    if not all(raw_dirs):
        pytest.skip("cached local PaddleOCR model directories are not configured")

    model_dirs = tuple(Path(value) for value in raw_dirs if value is not None)
    required_assets = ("inference.json", "inference.yml", "inference.pdiparams")
    missing = [
        model_dir / asset
        for model_dir in model_dirs
        for asset in required_assets
        if not (model_dir / asset).is_file()
    ]
    if missing:
        pytest.skip("cached local PaddleOCR model assets are incomplete")
    return model_dirs[0], model_dirs[1]


def test_adapts_paddle_result_without_numpy_values() -> None:
    spans = adapt_result(
        {
            "dt_polys": [[[1, 2], [5, 2], [5, 8], [1, 8]]],
            "rec_polys": [[[1, 2], [5, 2], [5, 8], [1, 8]]],
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


def test_adapts_recognition_aligned_polygons_not_detection_polygons() -> None:
    spans = adapt_result(
        {
            "dt_polys": [
                [[0, 0], [9, 0], [9, 9], [0, 9]],
                [[10, 0], [19, 0], [19, 9], [10, 9]],
            ],
            "rec_polys": [[[10, 0], [19, 0], [19, 9], [10, 9]]],
            "rec_texts": ["recognized"],
            "rec_scores": [0.9],
        }
    )

    assert len(spans) == 1
    assert spans[0].polygon == ((10, 0), (19, 0), (19, 9), (10, 9))


@pytest.mark.parametrize(
    "result",
    [
        {
            "dt_polys": [],
            "rec_polys": [],
            "rec_texts": ["orphan"],
            "rec_scores": [],
        },
        {
            "dt_polys": [[[1, 2], [5, 2], [5, 8], [1, 8]]],
            "rec_polys": [[[1, 2], [5, 2], [5, 8], [1, 8]]],
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
                    rec_polys=[[[1, 2], [5, 2], [5, 8], [1, 8]]],
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


def test_backend_serializes_concurrent_predict_calls() -> None:
    state_lock = threading.Lock()
    start_barrier = threading.Barrier(2)
    active_calls = 0
    maximum_active_calls = 0

    class OverlapDetectingPipeline:
        def predict(self, image: object) -> list[dict[str, object]]:
            del image
            nonlocal active_calls, maximum_active_calls
            with state_lock:
                active_calls += 1
                maximum_active_calls = max(maximum_active_calls, active_calls)
            time.sleep(0.05)
            with state_lock:
                active_calls -= 1
            return [
                {
                    "dt_polys": [],
                    "rec_polys": [],
                    "rec_texts": [],
                    "rec_scores": [],
                }
            ]

    pipeline = OverlapDetectingPipeline()
    backend = PaddleOcrBackend(pipeline_factory=lambda **kwargs: pipeline)
    image = Image.new("RGB", (10, 10), "white")

    def recognize_after_barrier() -> list[OcrSpan]:
        start_barrier.wait()
        return backend.recognize(image)

    try:
        with ThreadPoolExecutor(max_workers=2) as executor:
            results = list(
                executor.map(lambda _: recognize_after_barrier(), range(2))
            )
    finally:
        image.close()

    assert results == [[], []]
    assert maximum_active_calls == 1


def test_backend_uses_explicit_local_model_directories() -> None:
    calls: list[dict[str, object]] = []

    class FakePipeline:
        def predict(self, image: object) -> list[dict[str, object]]:
            del image
            return []

    detection_dir = Path("/cached/detection")
    recognition_dir = Path("/cached/recognition")
    PaddleOcrBackend(
        pipeline_factory=lambda **kwargs: (
            calls.append(kwargs) or FakePipeline()
        ),
        text_detection_model_dir=detection_dir,
        text_recognition_model_dir=recognition_dir,
    )

    assert calls == [
        {
            "device": "cpu",
            "use_doc_orientation_classify": False,
            "use_doc_unwarping": False,
            "use_textline_orientation": False,
            "text_detection_model_dir": str(detection_dir),
            "text_recognition_model_dir": str(recognition_dir),
        }
    ]


def test_backend_rejects_partial_local_model_configuration() -> None:
    with pytest.raises(ValueError, match="both local model directories"):
        PaddleOcrBackend(
            pipeline_factory=lambda **kwargs: pytest.fail(
                "pipeline must not initialize"
            ),
            text_detection_model_dir=Path("/cached/detection"),
        )


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
    detection_dir, recognition_dir = _require_cached_model_dirs()
    image = Image.new("RGB", (1400, 260), "white")
    draw = ImageDraw.Draw(image)
    font = ImageFont.truetype(
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", 72
    )
    draw.text((40, 70), "Cộng hòa Việt Nam", fill="black", font=font)

    try:
        spans = PaddleOcrBackend(
            text_detection_model_dir=detection_dir,
            text_recognition_model_dir=recognition_dir,
        ).recognize(image)
    finally:
        image.close()

    text = " ".join(span.text for span in spans).strip()
    assert text
    assert unicodedata.is_normalized("NFC", text)
