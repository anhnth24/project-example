#!/usr/bin/env python3
"""Bounded cell OCR and Markdown serialization for ruled-table experiments."""

from __future__ import annotations

import hashlib
import math
import re
import shutil
import tempfile
import time
import unicodedata
from dataclasses import dataclass, field
from pathlib import Path
from types import MappingProxyType
from typing import Any, Literal, Mapping, Sequence

from PIL import Image, ImageDraw

from benchmark.candidates import CommandCandidateSpec
from benchmark.corpus import BenchmarkPage
from benchmark.run import (
    CandidateOutputLimitError,
    _isolated_worker,
    sanitized_candidate_environment,
)
from experiments.table_lines import Grid, GridCell


FailureKind = Literal[
    "invalid_grid",
    "timeout",
    "output_limit",
    "candidate_error",
    "resource_limit",
]
_MAX_CELLS = 1_500
_MAX_OUTPUT_BYTES_PER_CELL = 65_536
_MAX_OUTPUT_BYTES_PER_PAGE = 1_048_576
_MAX_RSS_BYTES = 805_306_368
_MAX_PAGE_TIMEOUT_SECONDS = 20.0
_MAX_CELL_TIMEOUT_SECONDS = 10.0
_MAX_SAMPLE_INTERVAL_MS = 10
_HORIZONTAL_WHITESPACE = re.compile(r"[^\S\n]+")


def _is_int(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def _bounded_positive_number(
    value: object, *, name: str, maximum: float
) -> float:
    if (
        isinstance(value, bool)
        or not isinstance(value, (int, float))
        or not math.isfinite(float(value))
        or not 0 < float(value) <= maximum
    ):
        raise ValueError(f"{name} must be positive and at most {maximum:g}")
    return float(value)


def _bounded_positive_int(
    value: object, *, name: str, maximum: int
) -> int:
    if not _is_int(value) or not 0 < value <= maximum:
        raise ValueError(f"{name} must be positive and at most {maximum}")
    return value


@dataclass(frozen=True, slots=True)
class ProcessLimits:
    """Closed resource limits for one ruled-table page."""

    cpu_threads: int
    page_timeout_seconds: float
    cell_timeout_seconds: float
    max_output_bytes_per_cell: int
    max_output_bytes_per_page: int
    max_rss_bytes: int
    sample_interval_ms: int

    def __post_init__(self) -> None:
        if self.cpu_threads != 1 or not _is_int(self.cpu_threads):
            raise ValueError("cpu_threads must equal 1")
        object.__setattr__(
            self,
            "page_timeout_seconds",
            _bounded_positive_number(
                self.page_timeout_seconds,
                name="page_timeout_seconds",
                maximum=_MAX_PAGE_TIMEOUT_SECONDS,
            ),
        )
        object.__setattr__(
            self,
            "cell_timeout_seconds",
            _bounded_positive_number(
                self.cell_timeout_seconds,
                name="cell_timeout_seconds",
                maximum=_MAX_CELL_TIMEOUT_SECONDS,
            ),
        )
        for name, maximum in (
            ("max_output_bytes_per_cell", _MAX_OUTPUT_BYTES_PER_CELL),
            ("max_output_bytes_per_page", _MAX_OUTPUT_BYTES_PER_PAGE),
            ("max_rss_bytes", _MAX_RSS_BYTES),
            ("sample_interval_ms", _MAX_SAMPLE_INTERVAL_MS),
        ):
            object.__setattr__(
                self,
                name,
                _bounded_positive_int(
                    getattr(self, name), name=name, maximum=maximum
                ),
            )


@dataclass(frozen=True, slots=True)
class CellRecognition:
    row: int
    column: int
    text: str
    candidate_seconds: float
    resource: Mapping[str, Any] = field(repr=False, compare=False)

    def __post_init__(self) -> None:
        if (
            not _is_int(self.row)
            or not _is_int(self.column)
            or self.row < 0
            or self.column < 0
        ):
            raise ValueError("cell coordinates must be nonnegative integers")
        if not isinstance(self.text, str):
            raise TypeError("cell text must be a string")
        if (
            isinstance(self.candidate_seconds, bool)
            or not isinstance(self.candidate_seconds, (int, float))
            or not math.isfinite(float(self.candidate_seconds))
            or self.candidate_seconds < 0
        ):
            raise ValueError("candidate_seconds must be finite and nonnegative")
        try:
            resource = dict(self.resource)
        except (TypeError, ValueError) as error:
            raise TypeError("cell resource must be a mapping") from error
        object.__setattr__(self, "candidate_seconds", float(self.candidate_seconds))
        object.__setattr__(self, "resource", MappingProxyType(resource))


@dataclass(frozen=True, slots=True)
class GridRecognition:
    rows: int
    columns: int
    cells: tuple[CellRecognition, ...]

    def __post_init__(self) -> None:
        if (
            not _is_int(self.rows)
            or not _is_int(self.columns)
            or self.rows <= 0
            or self.columns <= 0
        ):
            raise ValueError("recognition dimensions must be positive integers")
        if not isinstance(self.cells, tuple):
            raise ValueError("recognition cells must be an immutable tuple")
        expected_count = self.rows * self.columns
        if expected_count > _MAX_CELLS or len(self.cells) > _MAX_CELLS:
            raise ValueError("recognition exceeds maximum 1500 cells")
        expected = tuple(
            (row, column)
            for row in range(self.rows)
            for column in range(self.columns)
        )
        actual = tuple((cell.row, cell.column) for cell in self.cells)
        if len(self.cells) != expected_count or actual != expected:
            raise ValueError(
                "recognition cells must be complete, unique, and row-major"
            )


class TableRecognitionError(RuntimeError):
    """Sanitized typed failure from bounded table-cell recognition."""

    def __init__(self, error_kind: FailureKind) -> None:
        self.error_kind = error_kind
        super().__init__(f"ruled-table recognition failed: {error_kind}")


class InvalidGridError(TableRecognitionError):
    def __init__(self) -> None:
        super().__init__("invalid_grid")


class PageOutputLimitError(TableRecognitionError):
    def __init__(self) -> None:
        super().__init__("output_limit")


def prepare_cell_crop(
    working_image: Image.Image,
    cell: GridCell,
    *,
    inset_pixels: int,
) -> Image.Image:
    """Return a detached grayscale crop with residual borders suppressed."""
    if not _is_int(inset_pixels) or inset_pixels < 0:
        raise ValueError("inset_pixels must be a nonnegative integer")
    if not isinstance(cell, GridCell):
        raise TypeError("cell must be a detected GridCell")
    width, height = working_image.size
    box = cell.working_box
    if (
        box.left < 0
        or box.top < 0
        or box.right > width
        or box.bottom > height
    ):
        raise ValueError("detected cell must be inside the working image")
    left = box.left + inset_pixels
    top = box.top + inset_pixels
    right = box.right - inset_pixels
    bottom = box.bottom - inset_pixels
    if right <= left or bottom <= top:
        raise ValueError("cell inset must leave a positive interior")

    cropped: Image.Image | None = None
    grayscale: Image.Image | None = None
    try:
        cropped = working_image.crop((left, top, right, bottom))
        grayscale = cropped.convert("L")
        draw = ImageDraw.Draw(grayscale)
        crop_width, crop_height = grayscale.size
        strip = min(2, crop_width, crop_height)
        if strip:
            draw.rectangle((0, 0, crop_width - 1, strip - 1), fill=255)
            draw.rectangle(
                (0, crop_height - strip, crop_width - 1, crop_height - 1),
                fill=255,
            )
            draw.rectangle((0, 0, strip - 1, crop_height - 1), fill=255)
            draw.rectangle(
                (crop_width - strip, 0, crop_width - 1, crop_height - 1),
                fill=255,
            )
        result = grayscale
        grayscale = None
        return result
    finally:
        if grayscale is not None:
            grayscale.close()
        if cropped is not None:
            cropped.close()


def is_blank_crop(crop: Image.Image) -> bool:
    """Return true only when at least 99.5% of pixels are near-white."""
    if crop.mode != "L":
        raise ValueError("blank detection requires a grayscale crop")
    total = crop.width * crop.height
    if total <= 0:
        raise ValueError("blank detection requires a positive-area crop")
    bright = sum(pixel >= 223 for pixel in crop.get_flattened_data())
    return bright * 1_000 >= total * 995


def hash_vie_traineddata(tessdata: Path) -> str:
    """Hash the exact local Vietnamese Tesseract model."""
    text = str(tessdata)
    if text.lower().startswith(("http:/", "https:/")):
        raise ValueError("tessdata must be an HTTPS-free local path")
    model = tessdata / "vie.traineddata"
    if not tessdata.is_dir() or not model.is_file():
        raise ValueError("local tessdata must contain vie.traineddata")
    digest = hashlib.sha256()
    with model.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def build_cell_candidate(
    *,
    candidate_id: str,
    psm: int,
    tessdata: Path,
    limits: ProcessLimits,
) -> CommandCandidateSpec:
    """Build one exact local Tesseract Vietnamese cell candidate."""
    if not _is_int(psm) or psm not in {6, 7}:
        raise ValueError("cell PSM must be 6 or 7")
    if not isinstance(tessdata, Path):
        raise TypeError("tessdata must be a Path")
    if not isinstance(limits, ProcessLimits):
        raise TypeError("limits must be ProcessLimits")
    tessdata_sha256 = hash_vie_traineddata(tessdata)
    environment = sanitized_candidate_environment(
        cpu_threads=limits.cpu_threads
    )
    environment["TESSDATA_PREFIX"] = str(tessdata)
    return CommandCandidateSpec(
        id=candidate_id,
        label=f"Tesseract vie PSM {psm}",
        argv=(
            "tesseract",
            "{input}",
            "stdout",
            "-l",
            "vie",
            "--psm",
            str(psm),
        ),
        environment=environment,
        provenance={
            "engine": "tesseract-cli",
            "langs": "vie",
            "psm": psm,
            "tessdata_sha256": tessdata_sha256,
        },
    )


def enforce_page_output_budget(
    output_sizes: Sequence[int], *, maximum: int
) -> None:
    """Enforce one additive page-output byte budget."""
    if not _is_int(maximum) or maximum <= 0:
        raise ValueError("maximum must be a positive integer")
    total = 0
    for size in output_sizes:
        if not _is_int(size) or size < 0:
            raise ValueError("output sizes must be nonnegative integers")
        total += size
        if total > maximum:
            raise PageOutputLimitError


def _normalize_cell_text(text: str) -> str:
    normalized = unicodedata.normalize("NFC", text)
    normalized = normalized.replace("\r\n", "\n").replace("\r", "\n")
    lines = [
        _HORIZONTAL_WHITESPACE.sub(" ", line).strip()
        for line in normalized.split("\n")
    ]
    while lines and not lines[0]:
        lines.pop(0)
    while lines and not lines[-1]:
        lines.pop()
    return "\n".join(lines)


def _failure(error_kind: FailureKind) -> TableRecognitionError:
    if error_kind == "invalid_grid":
        return InvalidGridError()
    if error_kind == "output_limit":
        return PageOutputLimitError()
    return TableRecognitionError(error_kind)


def recognize_grid(
    grid: Grid,
    *,
    working_image: Image.Image,
    candidate_id: str,
    cell_inset_pixels: int,
    psm: int,
    tessdata: Path,
    limits: ProcessLimits,
) -> GridRecognition:
    """Recognize one detected grid with one bounded isolated worker."""
    if not isinstance(grid, Grid):
        raise TypeError("grid must be a detected Grid")
    if len(grid.cells) > _MAX_CELLS:
        raise InvalidGridError()
    started = time.monotonic()
    deadline = started + limits.page_timeout_seconds
    spec = build_cell_candidate(
        candidate_id=candidate_id,
        psm=psm,
        tessdata=tessdata,
        limits=limits,
    )
    page_directory = Path(tempfile.mkdtemp(prefix="ruled-table-page-"))
    worker = None
    cells: list[CellRecognition] = []
    output_sizes: list[int] = []
    try:
        try:
            worker = _isolated_worker(
                spec,
                timeout_seconds=min(
                    limits.cell_timeout_seconds,
                    limits.page_timeout_seconds,
                ),
                max_output_bytes=limits.max_output_bytes_per_cell,
            )
        except Exception as error:
            raise _failure("candidate_error") from error

        for cell in grid.cells:
            crop_path = page_directory / (
                f"cell-{cell.row:04d}-{cell.column:04d}.png"
            )
            crop = prepare_cell_crop(
                working_image,
                cell,
                inset_pixels=cell_inset_pixels,
            )
            try:
                crop.save(crop_path, format="PNG")
                blank = is_blank_crop(crop)
            finally:
                crop.close()

            if blank:
                cells.append(
                    CellRecognition(
                        row=cell.row,
                        column=cell.column,
                        text="",
                        candidate_seconds=0.0,
                        resource={
                            "method": "skipped_blank_crop",
                            "peak_rss_bytes": 0,
                            "sample_count": 0,
                            "wall_seconds": 0.0,
                        },
                    )
                )
                continue

            before_request = time.monotonic()
            remaining = deadline - before_request
            if remaining <= 0:
                raise _failure("timeout")
            try:
                measurement = worker.recognize(
                    BenchmarkPage(
                        source_id="ruled-table-cell",
                        source_sha256="",
                        stratum="tuning",
                        page_number=cell.row * grid.columns + cell.column + 1,
                        path=crop_path,
                        reference=None,
                    ),
                    timeout_seconds=min(
                        limits.cell_timeout_seconds,
                        remaining,
                    ),
                )
            except TimeoutError as error:
                raise _failure("timeout") from error
            except CandidateOutputLimitError as error:
                raise _failure("output_limit") from error
            except TableRecognitionError:
                raise
            except Exception as error:
                raise _failure("candidate_error") from error

            if time.monotonic() >= deadline:
                raise _failure("timeout")
            raw_bytes = len(measurement.text.encode("utf-8"))
            if raw_bytes > limits.max_output_bytes_per_cell:
                raise _failure("output_limit")
            output_sizes.append(raw_bytes)
            enforce_page_output_budget(
                output_sizes,
                maximum=limits.max_output_bytes_per_page,
            )
            try:
                peak_rss = int(measurement.resource["peak_rss_bytes"])
            except (KeyError, TypeError, ValueError) as error:
                raise _failure("candidate_error") from error
            if peak_rss < 0:
                raise _failure("candidate_error")
            if peak_rss >= limits.max_rss_bytes:
                raise _failure("resource_limit")
            cells.append(
                CellRecognition(
                    row=cell.row,
                    column=cell.column,
                    text=_normalize_cell_text(measurement.text),
                    candidate_seconds=measurement.candidate_seconds,
                    resource=measurement.resource,
                )
            )
        return GridRecognition(
            rows=grid.rows,
            columns=grid.columns,
            cells=tuple(cells),
        )
    finally:
        try:
            if worker is not None:
                try:
                    worker.close()
                except Exception:
                    pass
        finally:
            shutil.rmtree(page_directory, ignore_errors=True)


def _markdown_cell(text: str) -> str:
    return text.strip().replace("\\", "\\\\").replace("|", "\\|").replace(
        "\n", "<br>"
    )


def serialize_markdown(recognition: GridRecognition) -> str:
    """Serialize a complete simple ruled table deterministically."""
    if recognition.rows < 2 or recognition.columns < 2:
        raise InvalidGridError()
    matrix = [
        recognition.cells[
            row * recognition.columns : (row + 1) * recognition.columns
        ]
        for row in range(recognition.rows)
    ]

    def row_text(row: Sequence[CellRecognition]) -> str:
        return "| " + " | ".join(_markdown_cell(cell.text) for cell in row) + " |\n"

    output = row_text(matrix[0])
    output += "|" + "|".join("---" for _ in range(recognition.columns)) + "|\n"
    output += "".join(row_text(row) for row in matrix[1:])
    return output
