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

import markhand_ocr.paddle_backend as paddle_backend  # noqa: E402
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


def _write_model_assets(model_dir: Path) -> Path:
    model_dir.mkdir()
    for asset in ("inference.json", "inference.yml", "inference.pdiparams"):
        (model_dir / asset).write_bytes(b"reviewed-local-asset")
    return model_dir


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


def test_backend_initializes_cpu_pipeline_once_and_adapts_predict_result(
    tmp_path: Path,
) -> None:
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

    detection_dir = _write_model_assets(tmp_path / "detection")
    recognition_dir = _write_model_assets(tmp_path / "recognition")
    backend = PaddleOcrBackend(
        pipeline_factory=factory,
        text_detection_model_dir=detection_dir,
        text_recognition_model_dir=recognition_dir,
    )
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
            "text_detection_model_dir": str(detection_dir.resolve()),
            "text_recognition_model_dir": str(recognition_dir.resolve()),
        }
    ]
    assert len(pipeline.images) == 2
    assert getattr(pipeline.images[0], "shape", None) == (10, 10, 3)
    assert first == second
    assert first[0].text == "Cộng hòa"
    assert backend.name == "paddle"


def test_backend_serializes_concurrent_predict_calls(tmp_path: Path) -> None:
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
    backend = PaddleOcrBackend(
        pipeline_factory=lambda **kwargs: pipeline,
        text_detection_model_dir=_write_model_assets(tmp_path / "detection"),
        text_recognition_model_dir=_write_model_assets(
            tmp_path / "recognition"
        ),
    )
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


def test_backend_uses_explicit_local_model_directories(tmp_path: Path) -> None:
    calls: list[dict[str, object]] = []

    class FakePipeline:
        def predict(self, image: object) -> list[dict[str, object]]:
            del image
            return []

    detection_dir = _write_model_assets(tmp_path / "detection")
    recognition_dir = _write_model_assets(tmp_path / "recognition")
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
            "text_detection_model_dir": str(detection_dir.resolve()),
            "text_recognition_model_dir": str(recognition_dir.resolve()),
        }
    ]


@pytest.mark.parametrize("configured", ["neither", "detection-only"])
def test_backend_rejects_any_non_cache_only_configuration(
    configured: str,
    tmp_path: Path,
) -> None:
    detection_dir = (
        _write_model_assets(tmp_path / "detection")
        if configured == "detection-only"
        else None
    )
    with pytest.raises(ValueError, match="local model directories are required"):
        PaddleOcrBackend(
            pipeline_factory=lambda **kwargs: pytest.fail(
                "pipeline must not initialize"
            ),
            text_detection_model_dir=detection_dir,
        )


def test_backend_rejects_incomplete_cache_before_pipeline_construction(
    tmp_path: Path,
) -> None:
    detection_dir = _write_model_assets(tmp_path / "detection")
    recognition_dir = _write_model_assets(tmp_path / "recognition")
    (recognition_dir / "inference.pdiparams").unlink()

    with pytest.raises(ValueError, match="cache is incomplete"):
        PaddleOcrBackend(
            pipeline_factory=lambda **kwargs: pytest.fail(
                "pipeline must not initialize"
            ),
            text_detection_model_dir=detection_dir,
            text_recognition_model_dir=recognition_dir,
        )


def test_runtime_selection_preserves_backend_injection_and_safe_health(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MARKHAND_OCR_BACKEND", "paddle")
    detection_dir = _write_model_assets(tmp_path / "detection")
    recognition_dir = _write_model_assets(tmp_path / "recognition")
    monkeypatch.setenv(
        "MARKHAND_OCR_DETECTION_MODEL_DIR", str(detection_dir)
    )
    monkeypatch.setenv(
        "MARKHAND_OCR_RECOGNITION_MODEL_DIR", str(recognition_dir)
    )
    monkeypatch.delenv("PADDLE_PDX_DISABLE_MODEL_SOURCE_CHECK", raising=False)
    backend = InjectedBackend()
    options: list[dict[str, object]] = []
    source_checks: list[str | None] = []

    response = TestClient(
        create_runtime_app(
            backend_factory=lambda **kwargs: (
                options.append(kwargs)
                or source_checks.append(
                    os.environ.get("PADDLE_PDX_DISABLE_MODEL_SOURCE_CHECK")
                )
                or backend
            )
        )
    ).get("/healthz")

    assert response.status_code == 200
    assert response.json() == {"status": "ready", "backend": "fake"}
    assert options == [
        {
            "text_detection_model_dir": detection_dir.resolve(),
            "text_recognition_model_dir": recognition_dir.resolve(),
        }
    ]
    assert source_checks == ["True"]


def test_runtime_rejects_unselected_backend(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("MARKHAND_OCR_BACKEND", raising=False)

    with pytest.raises(ValueError, match="MARKHAND_OCR_BACKEND"):
        create_runtime_app()


@pytest.mark.parametrize(
    ("missing", "message"),
    [
        ("both", "must be set"),
        ("recognition", "must be set"),
        ("asset", "incomplete"),
    ],
)
def test_runtime_fails_before_backend_construction_without_complete_local_cache(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    missing: str,
    message: str,
) -> None:
    monkeypatch.setenv("MARKHAND_OCR_BACKEND", "paddle")
    detection_dir = _write_model_assets(tmp_path / "detection")
    recognition_dir = _write_model_assets(tmp_path / "recognition")
    if missing != "both":
        monkeypatch.setenv(
            "MARKHAND_OCR_DETECTION_MODEL_DIR", str(detection_dir)
        )
    if missing not in {"both", "recognition"}:
        monkeypatch.setenv(
            "MARKHAND_OCR_RECOGNITION_MODEL_DIR", str(recognition_dir)
        )
    if missing == "asset":
        monkeypatch.setenv(
            "MARKHAND_OCR_RECOGNITION_MODEL_DIR", str(recognition_dir)
        )
        (recognition_dir / "inference.pdiparams").unlink()

    constructed = False

    def backend_factory(**kwargs: object) -> InjectedBackend:
        del kwargs
        nonlocal constructed
        constructed = True
        return InjectedBackend()

    with pytest.raises(ValueError, match=message):
        create_runtime_app(backend_factory=backend_factory)

    assert constructed is False


def test_runtime_applies_bounded_admission_timeouts_from_environment(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    detection_dir = _write_model_assets(tmp_path / "detection")
    recognition_dir = _write_model_assets(tmp_path / "recognition")
    monkeypatch.setenv("MARKHAND_OCR_BACKEND", "paddle")
    monkeypatch.setenv(
        "MARKHAND_OCR_DETECTION_MODEL_DIR", str(detection_dir)
    )
    monkeypatch.setenv(
        "MARKHAND_OCR_RECOGNITION_MODEL_DIR", str(recognition_dir)
    )
    monkeypatch.setenv("MARKHAND_OCR_ACQUISITION_TIMEOUT_SECONDS", "0.25")
    monkeypatch.setenv("MARKHAND_OCR_CONVERSION_DEADLINE_SECONDS", "45")
    app_options: list[dict[str, object]] = []
    sentinel = object()
    monkeypatch.setattr(
        paddle_backend,
        "create_app",
        lambda backend, **kwargs: (
            app_options.append({"backend": backend, **kwargs}) or sentinel
        ),
    )
    backend = InjectedBackend()

    result = create_runtime_app(backend_factory=lambda **kwargs: backend)

    assert result is sentinel
    assert app_options == [
        {
            "backend": backend,
            "acquisition_timeout_seconds": 0.25,
            "conversion_deadline_seconds": 45.0,
        }
    ]


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


@pytest.mark.live
@pytest.mark.skipif(
    os.environ.get("MARKHAND_OCR_LIVE") != "1",
    reason="set MARKHAND_OCR_LIVE=1 to run the cached-model smoke",
)
def test_live_runtime_app_starts_from_validated_cache_only(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    detection_dir, recognition_dir = _require_cached_model_dirs()
    monkeypatch.setenv("MARKHAND_OCR_BACKEND", "paddle")
    monkeypatch.setenv(
        "MARKHAND_OCR_DETECTION_MODEL_DIR", str(detection_dir)
    )
    monkeypatch.setenv(
        "MARKHAND_OCR_RECOGNITION_MODEL_DIR", str(recognition_dir)
    )

    with TestClient(create_runtime_app()) as client:
        response = client.get("/healthz")

    assert response.status_code == 200
    assert response.json() == {"status": "ready", "backend": "paddle"}
