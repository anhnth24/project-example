#!/usr/bin/env python3
"""Bounded cell OCR and Markdown serialization for ruled-table experiments."""

from __future__ import annotations

import hashlib
import math
import os
import re
import shutil
import stat
import tempfile
import time
import unicodedata
from dataclasses import dataclass, field
from pathlib import Path
from types import MappingProxyType
from typing import Any, Callable, Literal, Mapping, Sequence

from PIL import Image, ImageDraw

from benchmark.candidates import CommandCandidateSpec
from benchmark.corpus import BenchmarkPage
from benchmark.run import (
    CandidateOutputLimitError,
    CandidateResourceLimitError,
    CandidateResourceSamplingError,
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
    "cleanup_error",
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
    text: str = field(repr=False)
    elapsed_seconds: float
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
            isinstance(self.elapsed_seconds, bool)
            or not isinstance(self.elapsed_seconds, (int, float))
            or not math.isfinite(float(self.elapsed_seconds))
            or self.elapsed_seconds < 0
        ):
            raise ValueError("elapsed_seconds must be finite and nonnegative")
        try:
            resource = dict(self.resource)
        except (TypeError, ValueError) as error:
            raise TypeError("cell resource must be a mapping") from error
        object.__setattr__(self, "elapsed_seconds", float(self.elapsed_seconds))
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
        if self.rows > 50:
            raise ValueError("recognition rows exceeds maximum 50")
        if self.columns > 30:
            raise ValueError("recognition columns exceeds maximum 30")
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
        self.cleanup_failure: TableCleanupError | None = None
        super().__init__(f"ruled-table recognition failed: {error_kind}")

    def report_cleanup_failure(self, failure: TableCleanupError) -> None:
        self.cleanup_failure = failure
        self.args = (
            f"ruled-table recognition failed: {self.error_kind}; "
            "cleanup failed: cleanup_error",
        )


class InvalidGridError(TableRecognitionError):
    def __init__(self) -> None:
        super().__init__("invalid_grid")


class PageOutputLimitError(TableRecognitionError):
    def __init__(self) -> None:
        super().__init__("output_limit")


class TableCleanupError(TableRecognitionError):
    def __init__(self) -> None:
        super().__init__("cleanup_error")


@dataclass(frozen=True, slots=True)
class _PageDeadline:
    expires_at: float
    clock: Callable[[], float]

    def check(self) -> None:
        if self.clock() >= self.expires_at:
            raise TableRecognitionError("timeout")

    def remaining(self, *, maximum: float | None = None) -> float:
        remaining = self.expires_at - self.clock()
        if remaining <= 0:
            raise TableRecognitionError("timeout")
        return min(remaining, maximum) if maximum is not None else remaining


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


def hash_vie_traineddata(
    tessdata: Path,
    *,
    _deadline: _PageDeadline | None = None,
) -> str:
    """Hash the exact local Vietnamese Tesseract model."""
    if _deadline is not None:
        _deadline.check()
    text = str(tessdata)
    if text.lower().startswith(("http:/", "https:/")):
        raise ValueError("tessdata must be an HTTPS-free local path")
    model = tessdata / "vie.traineddata"
    if not tessdata.is_dir() or not model.is_file():
        raise ValueError("local tessdata must contain vie.traineddata")
    if _deadline is not None:
        _deadline.check()
    digest = hashlib.sha256()
    with model.open("rb") as stream:
        while True:
            if _deadline is not None:
                _deadline.check()
            chunk = stream.read(1024 * 1024)
            if _deadline is not None:
                _deadline.check()
            if not chunk:
                break
            digest.update(chunk)
    if _deadline is not None:
        _deadline.check()
    return digest.hexdigest()


def build_cell_candidate(
    *,
    candidate_id: str,
    psm: int,
    tessdata: Path,
    limits: ProcessLimits,
    _deadline: _PageDeadline | None = None,
) -> CommandCandidateSpec:
    """Build one exact local Tesseract Vietnamese cell candidate."""
    if _deadline is not None:
        _deadline.check()
    if not _is_int(psm) or psm not in {6, 7}:
        raise ValueError("cell PSM must be 6 or 7")
    if not isinstance(tessdata, Path):
        raise TypeError("tessdata must be a Path")
    if not isinstance(limits, ProcessLimits):
        raise TypeError("limits must be ProcessLimits")
    tessdata_sha256 = hash_vie_traineddata(
        tessdata,
        _deadline=_deadline,
    )
    if _deadline is not None:
        _deadline.check()
    environment = sanitized_candidate_environment(
        cpu_threads=limits.cpu_threads
    )
    environment["TESSDATA_PREFIX"] = str(tessdata)
    candidate = CommandCandidateSpec(
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
    if _deadline is not None:
        _deadline.check()
    return candidate


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


def _remove_owned_page_tree(path: Path) -> None:
    """Remove an owned tree without relying on shutil.rmtree."""
    try:
        path_stat = path.lstat()
    except FileNotFoundError:
        return
    if stat.S_ISLNK(path_stat.st_mode) or not stat.S_ISDIR(path_stat.st_mode):
        if not stat.S_ISLNK(path_stat.st_mode):
            os.chmod(
                path,
                path_stat.st_mode | stat.S_IRUSR | stat.S_IWUSR,
                follow_symlinks=False,
            )
        path.unlink()
        return

    os.chmod(
        path,
        path_stat.st_mode | stat.S_IRUSR | stat.S_IWUSR | stat.S_IXUSR,
        follow_symlinks=False,
    )
    with os.scandir(path) as entries:
        children = [Path(entry.path) for entry in entries]
    for child in children:
        _remove_owned_page_tree(child)
    path.rmdir()


def _owned_path_exists(path: Path) -> bool | None:
    try:
        path.lstat()
    except FileNotFoundError:
        return False
    except OSError:
        return None
    return True


def _cleanup_grid_resources(
    *,
    worker: Any,
    page_directory: Path | None,
    deadline: _PageDeadline,
) -> TableCleanupError | None:
    cleanup_failed = False
    if worker is not None:
        try:
            close_timeout = deadline.remaining()
        except TableRecognitionError:
            close_timeout = 0.0
        try:
            worker.close(timeout_seconds=close_timeout)
        except Exception:
            try:
                worker.force_kill()
            except Exception:
                cleanup_failed = True
    if page_directory is not None:
        try:
            shutil.rmtree(page_directory)
        except FileNotFoundError:
            pass
        except Exception:
            for _attempt in range(2):
                try:
                    _remove_owned_page_tree(page_directory)
                except FileNotFoundError:
                    break
                except Exception:
                    continue
                if _owned_path_exists(page_directory) is False:
                    break
        if _owned_path_exists(page_directory) is not False:
            cleanup_failed = True
    return TableCleanupError() if cleanup_failed else None


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
    deadline = _PageDeadline(
        expires_at=time.monotonic() + limits.page_timeout_seconds,
        clock=time.monotonic,
    )
    page_directory: Path | None = None
    worker = None
    result: GridRecognition | None = None
    primary_failure: TableRecognitionError | None = None
    cells: list[CellRecognition] = []
    output_sizes: list[int] = []
    try:
        try:
            deadline.check()
            spec = build_cell_candidate(
                candidate_id=candidate_id,
                psm=psm,
                tessdata=tessdata,
                limits=limits,
                _deadline=deadline,
            )
            deadline.check()
        except TableRecognitionError:
            raise
        except Exception:
            raise _failure("candidate_error") from None
        try:
            page_directory = Path(tempfile.mkdtemp(prefix="ruled-table-page-"))
            deadline.check()
        except TableRecognitionError:
            raise
        except Exception:
            raise _failure("candidate_error") from None
        try:
            worker = _isolated_worker(
                spec,
                timeout_seconds=deadline.remaining(
                    maximum=limits.cell_timeout_seconds,
                ),
                max_rss_bytes=limits.max_rss_bytes,
                max_output_bytes=limits.max_output_bytes_per_cell,
            )
            deadline.check()
        except TimeoutError:
            raise _failure("timeout") from None
        except (
            CandidateResourceLimitError,
            CandidateResourceSamplingError,
        ):
            raise _failure("resource_limit") from None
        except TableRecognitionError:
            raise
        except Exception:
            raise _failure("candidate_error") from None
        try:
            cold_peak_rss = int(
                worker.metadata["cold_initialization"]["rss_measurement"][
                    "peak_rss_bytes"
                ]
            )
        except (AttributeError, KeyError, TypeError, ValueError):
            raise _failure("candidate_error") from None
        if cold_peak_rss < 0:
            raise _failure("candidate_error")
        if cold_peak_rss >= limits.max_rss_bytes:
            raise _failure("resource_limit")
        deadline.check()

        for cell in grid.cells:
            deadline.check()
            if page_directory is None:
                raise _failure("candidate_error")
            crop_path = page_directory / (
                f"cell-{cell.row:04d}-{cell.column:04d}.png"
            )
            try:
                crop = prepare_cell_crop(
                    working_image,
                    cell,
                    inset_pixels=cell_inset_pixels,
                )
                deadline.check()
            except TableRecognitionError:
                raise
            except (TypeError, ValueError):
                raise _failure("invalid_grid") from None
            try:
                try:
                    crop.save(crop_path, format="PNG")
                    deadline.check()
                    blank = is_blank_crop(crop)
                    deadline.check()
                except TableRecognitionError:
                    raise
                except (OSError, ValueError):
                    raise _failure("candidate_error") from None
            finally:
                crop.close()
            deadline.check()

            if blank:
                cells.append(
                    CellRecognition(
                        row=cell.row,
                        column=cell.column,
                        text="",
                        elapsed_seconds=0.0,
                        resource={
                            "method": "skipped_blank_crop",
                            "peak_rss_bytes": 0,
                            "sample_count": 0,
                            "wall_seconds": 0.0,
                        },
                    )
                )
                deadline.check()
                continue

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
                    timeout_seconds=deadline.remaining(
                        maximum=limits.cell_timeout_seconds,
                    ),
                )
            except TimeoutError:
                raise _failure("timeout") from None
            except CandidateOutputLimitError:
                raise _failure("output_limit") from None
            except (
                CandidateResourceLimitError,
                CandidateResourceSamplingError,
            ):
                raise _failure("resource_limit") from None
            except TableRecognitionError:
                raise
            except Exception:
                raise _failure("candidate_error") from None

            deadline.check()
            raw_bytes = len(measurement.text.encode("utf-8"))
            if raw_bytes > limits.max_output_bytes_per_cell:
                raise _failure("output_limit")
            deadline.check()
            output_sizes.append(raw_bytes)
            enforce_page_output_budget(
                output_sizes,
                maximum=limits.max_output_bytes_per_page,
            )
            deadline.check()
            try:
                peak_rss = int(measurement.resource["peak_rss_bytes"])
                elapsed_seconds = float(
                    measurement.resource["wall_seconds"]
                )
            except (KeyError, TypeError, ValueError):
                raise _failure("candidate_error") from None
            if (
                peak_rss < 0
                or not math.isfinite(elapsed_seconds)
                or elapsed_seconds < 0
            ):
                raise _failure("candidate_error")
            if peak_rss >= limits.max_rss_bytes:
                raise _failure("resource_limit")
            deadline.check()
            normalized_text = _normalize_cell_text(measurement.text)
            deadline.check()
            cells.append(
                CellRecognition(
                    row=cell.row,
                    column=cell.column,
                    text=normalized_text,
                    elapsed_seconds=elapsed_seconds,
                    resource=measurement.resource,
                )
            )
            deadline.check()
        result = GridRecognition(
            rows=grid.rows,
            columns=grid.columns,
            cells=tuple(cells),
        )
        deadline.check()
    except TableRecognitionError as error:
        primary_failure = error
    except Exception:
        primary_failure = _failure("candidate_error")

    cleanup_failure = _cleanup_grid_resources(
        worker=worker,
        page_directory=page_directory,
        deadline=deadline,
    )
    try:
        deadline.check()
    except TableRecognitionError as error:
        if primary_failure is None:
            primary_failure = error

    if cleanup_failure is not None:
        if primary_failure is not None:
            primary_failure.report_cleanup_failure(cleanup_failure)
            raise primary_failure from None
        raise cleanup_failure from None
    if primary_failure is not None:
        raise primary_failure from None
    if result is None:
        raise _failure("candidate_error") from None
    return result


def _markdown_cell(text: str) -> str:
    return text.strip().replace("\\", "\\\\").replace("|", "\\|").replace(
        "\n", "<br>"
    )


def serialize_markdown(recognition: GridRecognition) -> str:
    """Serialize a complete simple ruled table deterministically."""
    if not isinstance(recognition, GridRecognition):
        raise InvalidGridError()
    try:
        GridRecognition(
            rows=recognition.rows,
            columns=recognition.columns,
            cells=recognition.cells,
        )
    except (TypeError, ValueError):
        raise InvalidGridError() from None
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
