"""Backend-independent OCR result models."""

from __future__ import annotations

from dataclasses import dataclass
from typing import TypeAlias

Point: TypeAlias = tuple[float, float]
Polygon: TypeAlias = tuple[Point, ...]


@dataclass(frozen=True, slots=True)
class OcrSpan:
    text: str
    confidence: float
    polygon: Polygon


@dataclass(frozen=True, slots=True)
class PageResult:
    page_number: int
    width: int
    height: int
    spans: tuple[OcrSpan, ...]
    backend: str
