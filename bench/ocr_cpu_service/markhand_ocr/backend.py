"""Backend boundary for OCR implementations."""

from __future__ import annotations

from typing import Protocol

from PIL import Image

from .models import OcrSpan


class OcrBackend(Protocol):
    """A named OCR engine that recognizes one already-rendered page."""

    name: str

    def recognize(self, image: Image.Image) -> list[OcrSpan]:
        """Return OCR spans using coordinates in rendered-image pixels."""

