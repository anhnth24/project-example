"""CPU-only PaddleOCR adapter using the documented public prediction fields."""

from __future__ import annotations

import os
import threading
import unicodedata
from collections.abc import Callable, Mapping, Sequence
from typing import Any, Protocol

import numpy as np
from fastapi import FastAPI
from PIL import Image

from .api import create_app
from .backend import OcrBackend
from .models import OcrSpan


class _PaddlePipeline(Protocol):
    def predict(self, image: np.ndarray[Any, Any]) -> Sequence[Mapping[str, Any]]:
        """Predict OCR results for one NumPy page image."""


PipelineFactory = Callable[..., _PaddlePipeline]


def _default_pipeline_factory(**kwargs: object) -> _PaddlePipeline:
    from paddleocr import PaddleOCR

    return PaddleOCR(**kwargs)


def adapt_result(result: Mapping[str, Any]) -> list[OcrSpan]:
    """Adapt PaddleOCR's documented recognition-aligned result fields."""
    polygons = result["rec_polys"]
    texts = result["rec_texts"]
    scores = result["rec_scores"]
    lengths = (len(polygons), len(texts), len(scores))
    if len(set(lengths)) != 1:
        raise ValueError(f"PaddleOCR result length mismatch: {lengths}")

    return [
        OcrSpan(
            text=unicodedata.normalize("NFC", str(text)),
            confidence=float(score),
            polygon=tuple(
                (float(point[0]), float(point[1])) for point in polygon
            ),
        )
        for polygon, text, score in zip(polygons, texts, scores, strict=True)
    ]


class PaddleOcrBackend:
    """One initialized PP-OCR pipeline, fixed to CPU inference."""

    name = "paddle"

    def __init__(
        self,
        *,
        pipeline_factory: PipelineFactory = _default_pipeline_factory,
        text_detection_model_dir: os.PathLike[str] | str | None = None,
        text_recognition_model_dir: os.PathLike[str] | str | None = None,
    ) -> None:
        if (text_detection_model_dir is None) != (
            text_recognition_model_dir is None
        ):
            raise ValueError("both local model directories must be provided")

        pipeline_options: dict[str, object] = {
            "device": "cpu",
            "use_doc_orientation_classify": False,
            "use_doc_unwarping": False,
            "use_textline_orientation": False,
        }
        if text_detection_model_dir is not None:
            pipeline_options.update(
                text_detection_model_dir=os.fspath(text_detection_model_dir),
                text_recognition_model_dir=os.fspath(
                    text_recognition_model_dir
                ),
            )
        self._pipeline = pipeline_factory(**pipeline_options)
        self._predict_lock = threading.Lock()

    def recognize(self, image: Image.Image) -> list[OcrSpan]:
        page = np.asarray(image)
        with self._predict_lock:
            spans: list[OcrSpan] = []
            for result in self._pipeline.predict(page):
                spans.extend(adapt_result(result))
        return spans


def create_runtime_app(
    *,
    backend_factory: Callable[[], OcrBackend] = PaddleOcrBackend,
) -> FastAPI:
    """Select and initialize the configured runtime backend exactly once."""
    if os.environ.get("MARKHAND_OCR_BACKEND") != "paddle":
        raise ValueError("MARKHAND_OCR_BACKEND must be set to paddle")
    return create_app(backend_factory())
