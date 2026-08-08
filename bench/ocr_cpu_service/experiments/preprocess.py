#!/usr/bin/env python3
"""Benchmark-only Tesseract shim and one-factor tuning matrix.

When selected through FILECONV_TESSERACT, this module receives the temporary
image already produced by fileconv's real Rust OCR pipeline.  It changes at
most one declared factor and invokes the real Tesseract with an argv array.
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import math
import os
import platform
import shutil
import signal
import statistics
import subprocess
import sys
import tempfile
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any, Iterator

import psutil
from PIL import Image, ImageFilter, ImageOps

from benchmark.candidates import CommandCandidateSpec
from benchmark.corpus import BenchmarkPage
from benchmark.metrics import error_counts
from benchmark.run import (
    CandidateOutputLimitError,
    _isolated_worker,
    sanitized_candidate_environment,
)
from corpus.split import load_accuracy_annotations
from experiments.run_matrix import (
    DEFAULT_ANNOTATIONS,
    DEFAULT_ASSETS,
    DEFAULT_SOURCES,
    ROOT,
    SERVICE_ROOT,
    _canonical_sha256,
    _host_description,
    _sha256,
    _toolchain_description,
    resolve_auto_tessdata,
    select_tuning_pages,
)

DEFAULT_CONFIGS = SERVICE_ROOT / "experiments" / "configs.json"
DEFAULT_BASELINE_RUN = (
    SERVICE_ROOT / ".data" / "accuracy-baseline" / "repetition-1.json"
)
DEFAULT_MATRIX_WORK = SERVICE_ROOT / ".data" / "accuracy-matrix"
DEFAULT_SHIM = Path(__file__).resolve()

MAX_DIMENSION = 10_000
MAX_PIXELS = 50_000_000
MAX_DESKEW_DEGREES = 3.0
PNG_COMPRESS_LEVEL = 9
EXPECTED_CONFIG_IDS = (
    "control",
    "dpi-hint-400",
    "deskew-auto-bounded",
    "threshold-otsu",
    "threshold-adaptive-pillow",
    "median-denoise",
    "background-watermark-suppression",
    "psm-4",
    "psm-6",
    "psm-11",
    "system-fast-tessdata",
)
_COUNT_FIELDS = (
    "character_edits",
    "reference_characters",
    "word_edits",
    "reference_words",
)
_CHECKSUM_FIELDS = (
    "source_sha256",
    "split_sha256",
    "configs_sha256",
    "binary_sha256",
    "shim_sha256",
    "baseline_config_sha256",
    "host_sha256",
    "toolchain_sha256",
    "tessdata_sha256",
)
_SHIM_ENVIRONMENT_NAMES = (
    "BENCH_OCR_EXPERIMENT_CONFIG_ID",
    "BENCH_OCR_EXPERIMENT_CONFIGS",
    "BENCH_OCR_EXPERIMENT_EVENTS",
    "BENCH_OCR_EXPERIMENT_WORK_DIR",
    "BENCH_OCR_REAL_TESSERACT",
    "FILECONV_TESSERACT",
)


@dataclass(frozen=True, slots=True)
class TransformResult:
    path: Path
    sha256: str
    width: int
    height: int
    deskew_degrees: float


def _canonical_json(value: Any) -> bytes:
    return json.dumps(
        value, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode()


def canonical_config_checksum(config: dict[str, Any]) -> str:
    public = {
        key: value for key, value in config.items() if key != "configuration_sha256"
    }
    return hashlib.sha256(_canonical_json(public)).hexdigest()


def load_experiment_configs(path: Path = DEFAULT_CONFIGS) -> dict[str, Any]:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"{path}: invalid experiment configs: {error}") from error
    if not isinstance(payload, dict) or set(payload) != {
        "schema_version",
        "split",
        "expected_pages",
        "limits",
        "configs",
    }:
        raise ValueError("experiment config fields are invalid")
    if (
        payload["schema_version"] != 1
        or payload["split"] != "tuning"
        or payload["expected_pages"] != 44
    ):
        raise ValueError("experiment configs must lock schema 1 and 44 tuning pages")
    configs = payload["configs"]
    if not isinstance(configs, list):
        raise ValueError("configs must be a list")
    ids = [item.get("id") for item in configs if isinstance(item, dict)]
    if len(ids) != len(set(ids)):
        raise ValueError("duplicate config ID")
    if ids.count("control") != 1:
        raise ValueError("exactly one control config is required")
    if tuple(ids) != EXPECTED_CONFIG_IDS:
        raise ValueError("config IDs or order are invalid")
    signatures: set[bytes] = set()
    for item in configs:
        if set(item) != {
            "id",
            "label",
            "changed_factor",
            "factor",
            "configuration_sha256",
        }:
            raise ValueError("config fields are invalid")
        factor = item["factor"]
        if not isinstance(factor, dict):
            raise ValueError("factor must be an object")
        expected_count = 0 if item["id"] == "control" else 1
        if len(factor) != expected_count:
            raise ValueError("each config must change exactly one factor")
        changed = item["changed_factor"]
        if item["id"] == "control":
            if changed is not None:
                raise ValueError("control cannot declare a changed factor")
        elif not isinstance(changed, str) or tuple(factor) != (changed,):
            raise ValueError("each config must encode one factor matching changed_factor")
        signature = _canonical_json(factor)
        if signature in signatures:
            raise ValueError("duplicate factor configuration")
        signatures.add(signature)
        if item["configuration_sha256"] != canonical_config_checksum(item):
            raise ValueError(f"configuration checksum mismatch: {item['id']}")
    limits = payload["limits"]
    if limits != {
        "cpu_threads": 1,
        "diagnostic_pages_per_candidate": 8,
        "max_output_bytes_per_stream": 1048576,
        "max_rss_bytes": 4294967296,
        "timeout_seconds_per_page": 180,
        "max_dimension": MAX_DIMENSION,
        "max_pixels": MAX_PIXELS,
        "max_deskew_degrees": MAX_DESKEW_DEGREES,
    }:
        raise ValueError("experiment limits are invalid")
    return payload


def _check_image_bounds(width: int, height: int) -> None:
    if width <= 0 or height <= 0:
        raise ValueError("image dimensions must be positive")
    if width > MAX_DIMENSION or height > MAX_DIMENSION:
        raise ValueError("image dimension exceeds experiment bound")
    if width * height > MAX_PIXELS:
        raise ValueError("image pixel count exceeds experiment bound")


def _deterministic_grayscale(image: Image.Image) -> Image.Image:
    return image.convert("L")


def _otsu(image: Image.Image) -> Image.Image:
    histogram = image.histogram()
    total = image.width * image.height
    weighted_total = sum(index * count for index, count in enumerate(histogram))
    background_weight = 0
    background_sum = 0
    best_variance = -1.0
    threshold = 0
    for value, count in enumerate(histogram):
        background_weight += count
        if background_weight == 0:
            continue
        foreground_weight = total - background_weight
        if foreground_weight == 0:
            break
        background_sum += value * count
        background_mean = background_sum / background_weight
        foreground_mean = (
            weighted_total - background_sum
        ) / foreground_weight
        variance = (
            background_weight
            * foreground_weight
            * (background_mean - foreground_mean) ** 2
        )
        if variance > best_variance:
            best_variance = variance
            threshold = value
    return image.point(lambda value: 255 if value > threshold else 0, mode="1").convert(
        "L"
    )


def _adaptive_pillow(image: Image.Image) -> Image.Image:
    # Pure Pillow: a fixed 31x31 local mean and a conservative 10-level offset.
    local_mean = image.filter(ImageFilter.BoxBlur(15))
    return Image.frombytes(
        "L",
        image.size,
        bytes(
            255 if pixel > mean - 10 else 0
            for pixel, mean in zip(
                image.getdata(), local_mean.getdata(), strict=True
            )
        ),
    )


def _background_suppression(image: Image.Image) -> Image.Image:
    # Preserve dark strokes; gently push only light background/watermark tones.
    return image.point(
        lambda value: value if value <= 180 else min(255, 180 + (value - 180) * 2)
    )


def _projection_score(image: Image.Image) -> int:
    binary = image.point(lambda value: 1 if value < 180 else 0)
    rows = [sum(binary.crop((0, y, binary.width, y + 1)).getdata()) for y in range(binary.height)]
    return sum((right - left) ** 2 for left, right in zip(rows, rows[1:]))


def _estimate_deskew(image: Image.Image) -> float:
    sample = image.copy()
    sample.thumbnail((1200, 1200), Image.Resampling.BILINEAR)
    candidates = [index / 4 for index in range(-12, 13)]
    scored = [
        (
            _projection_score(
                sample.rotate(
                    angle,
                    resample=Image.Resampling.BILINEAR,
                    expand=False,
                    fillcolor=255,
                )
            ),
            -abs(angle),
            -angle,
            angle,
        )
        for angle in candidates
    ]
    return max(scored)[-1]


def _rotate_expand_white(
    image: Image.Image, angle: float
) -> tuple[Image.Image, float]:
    bounded = max(-MAX_DESKEW_DEGREES, min(MAX_DESKEW_DEGREES, float(angle)))
    rotated = image.rotate(
        bounded,
        resample=Image.Resampling.BICUBIC,
        expand=True,
        fillcolor=255,
    )
    return rotated, bounded


def _apply_image_factor(
    image: Image.Image, factor: dict[str, Any]
) -> tuple[Image.Image, float]:
    gray = _deterministic_grayscale(image)
    deskew_degrees = 0.0
    if not factor:
        return gray, deskew_degrees
    name, settings = next(iter(factor.items()))
    if name in {"dpi_hint", "psm", "tessdata"}:
        return gray, deskew_degrees
    if name == "deskew":
        if settings != {
            "method": "projection-auto",
            "max_degrees": MAX_DESKEW_DEGREES,
            "step_degrees": 0.25,
            "expand": True,
            "fill": "white",
        }:
            raise ValueError("unsupported deskew settings")
        return _rotate_expand_white(gray, _estimate_deskew(gray))
    if name == "threshold":
        if settings == {"method": "otsu"}:
            return _otsu(gray), deskew_degrees
        if settings == {
            "method": "adaptive-pillow",
            "window": 31,
            "offset": 10,
        }:
            return _adaptive_pillow(gray), deskew_degrees
        raise ValueError("unsupported threshold settings")
    if name == "denoise":
        if settings != {"method": "median", "size": 3}:
            raise ValueError("unsupported denoise settings")
        return gray.filter(ImageFilter.MedianFilter(3)), deskew_degrees
    if name == "background_suppression":
        if settings != {
            "method": "light-tone-compress",
            "preserve_below": 180,
            "gain": 2,
        }:
            raise ValueError("unsupported background suppression settings")
        return _background_suppression(gray), deskew_degrees
    raise ValueError(f"unsupported experiment factor: {name}")


@contextlib.contextmanager
def transform_image(
    source: Path, factor: dict[str, Any], *, work_dir: Path
) -> Iterator[TransformResult]:
    work_dir.mkdir(parents=True, exist_ok=True)
    temporary: Path | None = None
    try:
        with Image.open(source) as opened:
            _check_image_bounds(opened.width, opened.height)
            opened.load()
            transformed, deskew_degrees = _apply_image_factor(opened, factor)
        _check_image_bounds(transformed.width, transformed.height)
        descriptor, name = tempfile.mkstemp(
            prefix="transform-", suffix=".png", dir=work_dir
        )
        os.close(descriptor)
        temporary = Path(name)
        transformed.save(
            temporary,
            format="PNG",
            optimize=False,
            compress_level=PNG_COMPRESS_LEVEL,
        )
        yield TransformResult(
            path=temporary,
            sha256=_sha256(temporary),
            width=transformed.width,
            height=transformed.height,
            deskew_degrees=deskew_degrees,
        )
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


@contextlib.contextmanager
def _passthrough_image(
    source: Path, *, work_dir: Path
) -> Iterator[TransformResult]:
    work_dir.mkdir(parents=True, exist_ok=True)
    with Image.open(source) as opened:
        _check_image_bounds(opened.width, opened.height)
        width, height = opened.size
    yield TransformResult(
        path=source,
        sha256=_sha256(source),
        width=width,
        height=height,
        deskew_degrees=0.0,
    )


def _replace_option(argv: list[str], option: str, value: str) -> None:
    try:
        index = argv.index(option)
    except ValueError:
        argv.extend([option, value])
    else:
        if index + 1 >= len(argv):
            raise ValueError(f"{option} has no value")
        argv[index + 1] = value


def _append_event(path: Path, event: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    encoded = _canonical_json(event) + b"\n"
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    try:
        os.write(descriptor, encoded)
    finally:
        os.close(descriptor)


def _run_real_tesseract(command: list[str]) -> int:
    child: subprocess.Popen[bytes] | None = None
    previous: dict[int, Any] = {}

    def forward(signum: int, _frame: Any) -> None:
        if child is not None and child.poll() is None:
            child.send_signal(signum)

    for signum in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
        previous[signum] = signal.signal(signum, forward)
    try:
        child = subprocess.Popen(command)
        return child.wait()
    finally:
        for signum, handler in previous.items():
            signal.signal(signum, handler)


def shim_main(argv: list[str] | None = None) -> int:
    args = list(sys.argv[1:] if argv is None else argv)
    real = os.environ.get("BENCH_OCR_REAL_TESSERACT")
    if not real:
        raise ValueError("BENCH_OCR_REAL_TESSERACT is required")
    if args == ["--version"]:
        return _run_real_tesseract([real, *args])
    if not args:
        raise ValueError("Tesseract image argv is required")
    config_id = os.environ.get("BENCH_OCR_EXPERIMENT_CONFIG_ID")
    if not config_id:
        raise ValueError("BENCH_OCR_EXPERIMENT_CONFIG_ID is required")
    configs_path = Path(
        os.environ.get("BENCH_OCR_EXPERIMENT_CONFIGS", str(DEFAULT_CONFIGS))
    )
    configs = load_experiment_configs(configs_path)
    config = next(
        (item for item in configs["configs"] if item["id"] == config_id), None
    )
    if config is None:
        raise ValueError("unknown experiment config ID")
    source = Path(args[0])
    input_sha256 = _sha256(source)
    factor = config["factor"]
    image_factor = (
        factor
        if config["changed_factor"]
        in {"deskew", "threshold", "denoise", "background_suppression"}
        else {}
    )
    work_dir = Path(
        os.environ.get(
            "BENCH_OCR_EXPERIMENT_WORK_DIR", str(DEFAULT_MATRIX_WORK / "shim")
        )
    )
    events = Path(os.environ["BENCH_OCR_EXPERIMENT_EVENTS"])
    previous: dict[int, Any] = {}

    def abort_during_setup(signum: int, _frame: Any) -> None:
        raise SystemExit(128 + signum)

    for signum in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
        previous[signum] = signal.signal(signum, abort_during_setup)
    try:
        image_context = (
            transform_image(source, image_factor, work_dir=work_dir)
            if image_factor
            else _passthrough_image(source, work_dir=work_dir)
        )
        with image_context as transformed:
            command_args = list(args)
            command_args[0] = str(transformed.path)
            if config["changed_factor"] == "dpi_hint":
                _replace_option(
                    command_args,
                    "--dpi",
                    str(config["factor"]["dpi_hint"]["value"]),
                )
            elif config["changed_factor"] == "psm":
                _replace_option(
                    command_args, "--psm", str(config["factor"]["psm"]["value"])
                )
            _append_event(
                events,
                {
                    "config_id": config_id,
                    "configuration_sha256": config["configuration_sha256"],
                    "input_sha256": input_sha256,
                    "transform_sha256": transformed.sha256,
                    "width": transformed.width,
                    "height": transformed.height,
                    "deskew_degrees": transformed.deskew_degrees,
                },
            )
            return _run_real_tesseract([real, *command_args])
    finally:
        for signum, handler in previous.items():
            signal.signal(signum, handler)


def aggregate_matrix_records(records: list[dict[str, Any]]) -> dict[str, Any]:
    successful = [record for record in records if record["success"]]
    raw = {
        field: sum(int(record[field]) for record in successful)
        for field in _COUNT_FIELDS
    }
    elapsed = sorted(float(record["elapsed_seconds"]) for record in successful)
    rss = [int(record["peak_rss_bytes"]) for record in successful]
    return {
        "pages": len(records),
        "successes": len(successful),
        "failures": len(records) - len(successful),
        "raw_counts": raw,
        "cer": (
            raw["character_edits"] / raw["reference_characters"]
            if raw["reference_characters"]
            else 0.0
        ),
        "wer": (
            raw["word_edits"] / raw["reference_words"]
            if raw["reference_words"]
            else 0.0
        ),
        "latency_seconds": {
            "median": statistics.median(elapsed) if elapsed else 0.0,
            "p95": (
                elapsed[max(0, math.ceil(0.95 * len(elapsed)) - 1)]
                if elapsed
                else 0.0
            ),
            "total": sum(elapsed),
        },
        "peak_rss_bytes": max(rss, default=0),
        "resource_limit_violations": sum(
            bool(record["resource_limit_violation"]) for record in records
        ),
    }


def _event_summary(events_path: Path) -> tuple[str, int, float]:
    if not events_path.exists():
        raise ValueError("shim emitted no transform event")
    events = [
        json.loads(line)
        for line in events_path.read_text(encoding="utf-8").splitlines()
        if line
    ]
    if not events:
        raise ValueError("shim emitted no transform event")
    checksum = hashlib.sha256(
        _canonical_json([event["transform_sha256"] for event in events])
    ).hexdigest()
    angle = max(abs(float(event["deskew_degrees"])) for event in events)
    return checksum, len(events), angle


def _run_matrix_candidate(
    spec: CommandCandidateSpec,
    public: dict[str, Any],
    pages: list[dict[str, Any]],
    *,
    events_path: Path,
    timeout_seconds: float,
    max_output_bytes: int,
    max_rss_bytes: int,
    diagnostic_limit: int,
) -> dict[str, Any]:
    worker = _isolated_worker(
        spec,
        timeout_seconds=timeout_seconds,
        max_output_bytes=max_output_bytes,
    )
    records: list[dict[str, Any]] = []
    try:
        for page in pages:
            events_path.unlink(missing_ok=True)
            benchmark_page = BenchmarkPage(
                source_id=page["source_id"],
                source_sha256=page["source_sha256"],
                stratum=page["document_type"],
                page_number=page["page_number"],
                path=page["path"],
                reference=None,
            )
            record: dict[str, Any] = {
                "page_id": page["page_id"],
                "split": "tuning",
                "source_id": page["source_id"],
                "page_number": page["page_number"],
                "document_type": page["document_type"],
                "difficulty_strata": sorted(page["difficulty_strata"]),
                "success": False,
                "error_kind": None,
                "elapsed_seconds": 0.0,
                "peak_rss_bytes": 0,
                "resource_limit_violation": False,
            }
            try:
                measurement = worker.recognize(benchmark_page)
                counts = error_counts(page["transcription"], measurement.text)
                transform_sha256, attempts, angle = _event_summary(events_path)
                peak_rss = int(measurement.resource["peak_rss_bytes"])
                record.update(
                    success=True,
                    character_edits=counts.character_edits,
                    reference_characters=counts.reference_characters,
                    word_edits=counts.word_edits,
                    reference_words=counts.reference_words,
                    elapsed_seconds=float(measurement.resource["wall_seconds"]),
                    candidate_seconds=float(measurement.candidate_seconds),
                    peak_rss_bytes=peak_rss,
                    rss_sample_count=int(measurement.resource["sample_count"]),
                    resource_limit_violation=peak_rss > max_rss_bytes,
                    transform_sha256=transform_sha256,
                    transform_attempts=attempts,
                    max_abs_deskew_degrees=angle,
                )
            except TimeoutError:
                record["error_kind"] = "timeout"
            except CandidateOutputLimitError:
                record["error_kind"] = "output_limit"
            except Exception:
                record["error_kind"] = "candidate_error"
            records.append(record)
            print(
                f"[{spec.id}] {page['page_id']}: "
                f"{'ok' if record['success'] else record['error_kind']}",
                file=sys.stderr,
                flush=True,
            )
    finally:
        worker.close()
        events_path.unlink(missing_ok=True)
    strata = {
        stratum: aggregate_matrix_records(
            [
                record
                for record in records
                if stratum in record["difficulty_strata"]
            ]
        )
        for stratum in sorted(
            {
                stratum
                for record in records
                for stratum in record["difficulty_strata"]
            }
        )
    }
    document_types = {
        kind: aggregate_matrix_records(
            [record for record in records if record["document_type"] == kind]
        )
        for kind in sorted({record["document_type"] for record in records})
    }
    diagnostics = sorted(
        records,
        key=lambda record: (
            -record.get("character_edits", 0)
            / max(1, record.get("reference_characters", 0)),
            record["page_id"],
        ),
    )[:diagnostic_limit]
    return {
        **public,
        "aggregate": aggregate_matrix_records(records),
        "strata": strata,
        "document_types": document_types,
        "records": records,
        "diagnostics": diagnostics,
    }


def _valid_checksum(value: Any) -> bool:
    return isinstance(value, str) and len(value) == 64 and all(
        character in "0123456789abcdef" for character in value
    )


def validate_matrix_artifact(
    artifact: dict[str, Any], configs: dict[str, Any]
) -> None:
    if set(artifact) != {
        "schema_version",
        "split",
        "page_count",
        "provenance",
        "access",
        "baseline_calibration",
        "candidates",
    }:
        raise ValueError("matrix artifact fields are invalid")
    if (
        artifact["schema_version"] != 1
        or artifact["split"] != "tuning"
        or artifact["page_count"] != 44
    ):
        raise ValueError("matrix artifact must contain exactly 44 tuning pages")
    access = artifact["access"]
    if access != {
        "selected_tuning_pages": 44,
        "tuning_assets_checksummed": 44,
        "holdout_assets_resolved": 0,
        "holdout_assets_checksummed": 0,
        "holdout_assets_opened": 0,
        "holdout_ocr_executions": 0,
    }:
        raise ValueError("holdout non-access evidence is invalid")
    provenance = artifact["provenance"]
    if set(provenance) != set(_CHECKSUM_FIELDS):
        raise ValueError("matrix provenance is incomplete")
    for key, value in provenance.items():
        if key == "tessdata_sha256":
            if set(value) != {"system-fast", "best"} or not all(
                set(digests) == {"vie", "eng"}
                and all(_valid_checksum(digest) for digest in digests.values())
                for digests in value.values()
            ):
                raise ValueError("tessdata checksums are invalid")
        elif not _valid_checksum(value):
            raise ValueError(f"invalid provenance checksum: {key}")
    calibration = artifact["baseline_calibration"]
    if calibration != {
        "expected_character_edits": 6707,
        "expected_reference_characters": 54897,
        "expected_word_edits": 3383,
        "expected_reference_words": 11959,
        "markhand_auto_match": True,
        "explicit_best_match": True,
    }:
        raise ValueError("control calibration did not match both best baselines")
    expected = configs["configs"]
    candidates = artifact["candidates"]
    if [candidate.get("id") for candidate in candidates] != [
        config["id"] for config in expected
    ]:
        raise ValueError("matrix candidate IDs are invalid")
    expected_pages: set[str] | None = None
    for candidate, config in zip(candidates, expected, strict=True):
        if (
            candidate["changed_factor"] != config["changed_factor"]
            or candidate["factor"] != config["factor"]
            or candidate["configuration_sha256"]
            != config["configuration_sha256"]
        ):
            raise ValueError("candidate configuration provenance drift")
        names = candidate["environment_variable_names"]
        if (
            not isinstance(names, list)
            or any(not isinstance(name, str) for name in names)
            or "FILECONV_TESSERACT" not in names
            or "BENCH_OCR_EXPERIMENT_CONFIG_ID" not in names
        ):
            raise ValueError("shim environment-variable names are incomplete")
        records = candidate["records"]
        page_ids = [record.get("page_id") for record in records]
        if len(records) != 44 or len(set(page_ids)) != 44:
            raise ValueError("every config requires 44 unique tuning records")
        if expected_pages is None:
            expected_pages = set(page_ids)
        elif set(page_ids) != expected_pages:
            raise ValueError("configs did not run on identical tuning pages")
        for record in records:
            if record.get("split") != "tuning":
                raise ValueError("holdout matrix record is forbidden")
            if record["success"]:
                if not _valid_checksum(record.get("transform_sha256")):
                    raise ValueError("transform checksum is invalid")
                if int(record.get("transform_attempts", 0)) <= 0:
                    raise ValueError("transform attempts are invalid")
            elif not record.get("error_kind"):
                raise ValueError("failure diagnostics are invalid")
        if candidate["aggregate"] != aggregate_matrix_records(records):
            raise ValueError("matrix aggregate does not match raw records")
        if len(candidate["diagnostics"]) > 8:
            raise ValueError("bounded diagnostics limit exceeded")
        for field, memberships in (
            ("strata", "difficulty_strata"),
            ("document_types", "document_type"),
        ):
            expected_keys = (
                {
                    item
                    for record in records
                    for item in record[memberships]
                }
                if memberships == "difficulty_strata"
                else {record[memberships] for record in records}
            )
            if set(candidate[field]) != expected_keys:
                raise ValueError(f"{field} are incomplete")
            for key, aggregate in candidate[field].items():
                rows = [
                    record
                    for record in records
                    if (
                        key in record[memberships]
                        if memberships == "difficulty_strata"
                        else key == record[memberships]
                    )
                ]
                if aggregate != aggregate_matrix_records(rows):
                    raise ValueError(f"{field} aggregate mismatch")
    serialized = json.dumps(artifact).lower()
    for forbidden in (
        "recognized_text",
        "recognised_text",
        '"hypothesis"',
        '"transcription"',
        '"reference"',
        '"stdout"',
        '"stderr"',
        '"environment"',
    ):
        if forbidden in serialized:
            raise ValueError("recognized text or environment values are forbidden")


def _format_aggregate_row(config_id: str, aggregate: dict[str, Any]) -> str:
    raw = aggregate["raw_counts"]
    return (
        f"| `{config_id}` | {aggregate['pages']} | {raw['character_edits']} / "
        f"{raw['reference_characters']} | {aggregate['cer']:.6f} | "
        f"{raw['word_edits']} / {raw['reference_words']} | "
        f"{aggregate['wer']:.6f} | "
        f"{aggregate['latency_seconds']['median']:.6f} | "
        f"{aggregate['latency_seconds']['p95']:.6f} | "
        f"{aggregate['peak_rss_bytes'] / 1048576:.2f} | "
        f"{aggregate['failures']} |"
    )


def render_matrix_report(artifact: dict[str, Any]) -> str:
    ranked = sorted(
        artifact["candidates"],
        key=lambda candidate: (candidate["aggregate"]["cer"], candidate["id"]),
    )
    lines = [
        "# Vietnamese OCR one-factor tuning matrix",
        "",
        "## Scope and interpretation",
        "",
        "Current best baseline: **12.2174% CER** (`markhand-auto` and explicit "
        "`tessdata-best`, 6,707 / 54,897 character edits).",
        "",
        "The control uses the benchmark-only Tesseract shim as a strict passthrough "
        "after fileconv performs its normal Rust image decode, conditional resize, "
        "grayscale, unsharp mask, normalization, column detection, retry scoring, "
        "and temporary-file lifecycle. Matrix transferability is accepted only "
        "because control counts calibrated exactly to both Task 2 best baselines.",
        "",
        "The `dpi-hint-400` factor changes only Tesseract's DPI hint. Source images "
        "prevent a true render-resolution experiment, so this is **not a 300/400 "
        "PDF rerender comparison**.",
        "",
        "**No holdout result or policy selection is included.** Holdout assets were "
        "not resolved, checksummed, opened, or executed.",
        "",
        "Ranks use tuning-set micro-average CER. Document-type and difficulty "
        "small strata are descriptive, overlap where noted, and are not population "
        "estimates.",
        "",
        "Tracked rows contain additive edit counts and transform checksums, never "
        "document content. Only environment variable names are "
        "published; arbitrary values are omitted.",
        "",
        "## Ranked overall tuning measurements",
        "",
        "| Rank | Config | Changed factor | Character edits / chars | CER | "
        "Word edits / words | WER | Median s/page | p95 s/page | Peak RSS MiB | "
        "Failures |",
        "| ---: | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
    ]
    for rank, candidate in enumerate(ranked, 1):
        aggregate = candidate["aggregate"]
        raw = aggregate["raw_counts"]
        lines.append(
            f"| {rank} | `{candidate['id']}` | "
            f"`{candidate['changed_factor'] or 'none'}` | "
            f"{raw['character_edits']} / {raw['reference_characters']} | "
            f"{aggregate['cer']:.6f} | {raw['word_edits']} / "
            f"{raw['reference_words']} | {aggregate['wer']:.6f} | "
            f"{aggregate['latency_seconds']['median']:.6f} | "
            f"{aggregate['latency_seconds']['p95']:.6f} | "
            f"{aggregate['peak_rss_bytes'] / 1048576:.2f} | "
            f"{aggregate['failures']} |"
        )
    lines.extend(
        [
            "",
            "## Exact one-factor configurations",
            "",
            "| Config | Changed factor | Published value | Configuration SHA-256 |",
            "| --- | --- | --- | --- |",
        ]
    )
    for candidate in artifact["candidates"]:
        lines.append(
            f"| `{candidate['id']}` | "
            f"`{candidate['changed_factor'] or 'none'}` | "
            f"`{json.dumps(candidate['factor'], sort_keys=True, separators=(',', ':'))}` | "
            f"`{candidate['configuration_sha256']}` |"
        )
    lines.extend(
        [
            "",
            "## Document-type strata",
            "",
            "The 9-page `modern-government` stratum and 35-page "
            "`historical-old-print` stratum are reported separately.",
            "",
            "| Config | Document type | Pages | Character edits / chars | CER | "
            "Word edits / words | WER |",
            "| --- | --- | ---: | ---: | ---: | ---: | ---: |",
        ]
    )
    for candidate in artifact["candidates"]:
        for kind, aggregate in candidate["document_types"].items():
            raw = aggregate["raw_counts"]
            lines.append(
                f"| `{candidate['id']}` | `{kind}` | {aggregate['pages']} | "
                f"{raw['character_edits']} / {raw['reference_characters']} | "
                f"{aggregate['cer']:.6f} | {raw['word_edits']} / "
                f"{raw['reference_words']} | {aggregate['wer']:.6f} |"
            )
    lines.extend(
        [
            "",
            "## Overlapping difficulty strata",
            "",
            "| Config | Difficulty | Pages | Character edits / chars | CER | "
            "Word edits / words | WER |",
            "| --- | --- | ---: | ---: | ---: | ---: | ---: |",
        ]
    )
    for candidate in artifact["candidates"]:
        for stratum, aggregate in candidate["strata"].items():
            raw = aggregate["raw_counts"]
            lines.append(
                f"| `{candidate['id']}` | `{stratum}` | {aggregate['pages']} | "
                f"{raw['character_edits']} / {raw['reference_characters']} | "
                f"{aggregate['cer']:.6f} | {raw['word_edits']} / "
                f"{raw['reference_words']} | {aggregate['wer']:.6f} |"
            )
    lines.extend(
        [
            "",
            "## Raw additive counts and transform checksums",
            "",
            "| Config | Page ID | Character edits | Chars | Word edits | Words | "
            "Transform SHA-256 | Attempts |",
            "| --- | --- | ---: | ---: | ---: | ---: | --- | ---: |",
        ]
    )
    for candidate in artifact["candidates"]:
        for record in candidate["records"]:
            lines.append(
                f"| `{candidate['id']}` | {record['page_id']} | "
                f"{record.get('character_edits', 0)} | "
                f"{record.get('reference_characters', 0)} | "
                f"{record.get('word_edits', 0)} | "
                f"{record.get('reference_words', 0)} | "
                f"`{record.get('transform_sha256', '')}` | "
                f"{record.get('transform_attempts', 0)} |"
            )
    names = sorted(
        {
            name
            for candidate in artifact["candidates"]
            for name in candidate["environment_variable_names"]
        }
    )
    lines.extend(
        [
            "",
            "## Provenance, bounds, and non-access evidence",
            "",
            "Environment variable names: "
            + ", ".join(f"`{name}`" for name in names)
            + ". Values are intentionally not serialized.",
            "",
            f"- Config file SHA-256: `{artifact['provenance']['configs_sha256']}`.",
            f"- Shim SHA-256: `{artifact['provenance']['shim_sha256']}`.",
            f"- Fileconv SHA-256: `{artifact['provenance']['binary_sha256']}`.",
            "- Maximum input/output dimensions: 10,000 px; maximum pixels: "
            "50,000,000; maximum absolute deskew: 3.0 degrees; deskew expands "
            "onto white padding and never crops.",
            "- Each page has a 180-second deadline. Candidate stdout and stderr "
            "are independently capped at 1,048,576 bytes; timeout/overflow "
            "terminates the process tree.",
            "- RSS is a 10 ms sampled process-tree sum with a measured 4 GiB gate, "
            "not a kernel-enforced memory limit.",
            "- Shim transform files are removed on success, failure, and handled "
            "termination signals. The real Tesseract is invoked by direct argv "
            "without a shell.",
            "",
            "| Selected tuning | Tuning checksums | Holdout resolved | Holdout "
            "checksums | Holdout opened | Holdout OCR |",
            "| ---: | ---: | ---: | ---: | ---: | ---: |",
            f"| {artifact['access']['selected_tuning_pages']} | "
            f"{artifact['access']['tuning_assets_checksummed']} | "
            f"{artifact['access']['holdout_assets_resolved']} | "
            f"{artifact['access']['holdout_assets_checksummed']} | "
            f"{artifact['access']['holdout_assets_opened']} | "
            f"{artifact['access']['holdout_ocr_executions']} |",
            "",
        ]
    )
    return "\n".join(lines)


def _build_specs(
    configs: dict[str, Any],
    *,
    fileconv: Path,
    shim: Path,
    real_tesseract: Path,
    system_tessdata: Path,
    work_dir: Path,
) -> tuple[list[CommandCandidateSpec], list[dict[str, Any]], list[Path]]:
    base = sanitized_candidate_environment(
        cpu_threads=configs["limits"]["cpu_threads"]
    )
    specs: list[CommandCandidateSpec] = []
    public: list[dict[str, Any]] = []
    events_paths: list[Path] = []
    for config in configs["configs"]:
        events = work_dir / "events" / f"{config['id']}.jsonl"
        environment = {
            **base,
            "FILECONV_TESSERACT": str(shim),
            "BENCH_OCR_REAL_TESSERACT": str(real_tesseract),
            "BENCH_OCR_EXPERIMENT_CONFIG_ID": config["id"],
            "BENCH_OCR_EXPERIMENT_CONFIGS": str(DEFAULT_CONFIGS.resolve()),
            "BENCH_OCR_EXPERIMENT_EVENTS": str(events),
            "BENCH_OCR_EXPERIMENT_WORK_DIR": str(
                work_dir / "transforms" / config["id"]
            ),
        }
        if config["changed_factor"] == "tessdata":
            environment["FILECONV_TESSDATA"] = str(system_tessdata)
        spec = CommandCandidateSpec(
            id=config["id"],
            label=config["label"],
            argv=(
                str(fileconv),
                "one",
                "{input}",
                "--lang",
                "vie+eng",
            ),
            environment=environment,
            provenance={},
        )
        specs.append(spec)
        public.append(
            {
                "id": config["id"],
                "label": config["label"],
                "changed_factor": config["changed_factor"],
                "factor": config["factor"],
                "configuration_sha256": config["configuration_sha256"],
                "environment_variable_names": sorted(environment),
            }
        )
        events_paths.append(events)
    return specs, public, events_paths


def run_tuning_matrix(args: argparse.Namespace) -> dict[str, Any]:
    configs_path = args.configs.resolve()
    configs = load_experiment_configs(configs_path)
    rows = load_accuracy_annotations(args.annotations.resolve())
    pages, split_payload = select_tuning_pages(
        rows,
        assets_dir=args.assets_dir.resolve(),
        expected_pages=configs["expected_pages"],
    )
    fileconv = args.fileconv.resolve()
    shim = args.shim.resolve()
    real_tesseract = args.real_tesseract.resolve()
    system_tessdata = args.system_tessdata.resolve()
    best_tessdata = resolve_auto_tessdata(
        cwd=Path.cwd(),
        executable=fileconv,
        manifest_dir=ROOT / "crates" / "core",
    )
    host = _host_description(configs["limits"]["max_rss_bytes"])
    toolchain = _toolchain_description()
    provenance = {
        "source_sha256": _sha256(args.sources.resolve()),
        "split_sha256": hashlib.sha256(split_payload).hexdigest(),
        "configs_sha256": _sha256(configs_path),
        "binary_sha256": _sha256(fileconv),
        "shim_sha256": _sha256(shim),
        "baseline_config_sha256": _sha256(
            SERVICE_ROOT / "experiments" / "baseline.json"
        ),
        "host_sha256": _canonical_sha256(host),
        "toolchain_sha256": _canonical_sha256(toolchain),
        "tessdata_sha256": {
            role: {
                language: _sha256(path / f"{language}.traineddata")
                for language in ("vie", "eng")
            }
            for role, path in {
                "system-fast": system_tessdata,
                "best": best_tessdata,
            }.items()
        },
    }
    specs, public, event_paths = _build_specs(
        configs,
        fileconv=fileconv,
        shim=shim,
        real_tesseract=real_tesseract,
        system_tessdata=system_tessdata,
        work_dir=args.work_dir.resolve(),
    )
    candidates = [
        _run_matrix_candidate(
            spec,
            public_item,
            pages,
            events_path=events,
            timeout_seconds=configs["limits"]["timeout_seconds_per_page"],
            max_output_bytes=configs["limits"]["max_output_bytes_per_stream"],
            max_rss_bytes=configs["limits"]["max_rss_bytes"],
            diagnostic_limit=configs["limits"]["diagnostic_pages_per_candidate"],
        )
        for spec, public_item, events in zip(
            specs, public, event_paths, strict=True
        )
    ]
    baseline = json.loads(args.baseline_run.read_text(encoding="utf-8"))
    baseline_counts = {
        candidate["id"]: candidate["aggregate"]["raw_counts"]
        for candidate in baseline["candidates"]
    }
    expected = {
        "character_edits": 6707,
        "reference_characters": 54897,
        "word_edits": 3383,
        "reference_words": 11959,
    }
    control_counts = candidates[0]["aggregate"]["raw_counts"]
    artifact = {
        "schema_version": 1,
        "split": "tuning",
        "page_count": len(pages),
        "provenance": provenance,
        "access": {
            "selected_tuning_pages": len(pages),
            "tuning_assets_checksummed": len(pages),
            "holdout_assets_resolved": 0,
            "holdout_assets_checksummed": 0,
            "holdout_assets_opened": 0,
            "holdout_ocr_executions": 0,
        },
        "baseline_calibration": {
            "expected_character_edits": 6707,
            "expected_reference_characters": 54897,
            "expected_word_edits": 3383,
            "expected_reference_words": 11959,
            "markhand_auto_match": (
                control_counts == expected
                and baseline_counts.get("markhand-auto") == expected
            ),
            "explicit_best_match": (
                control_counts == expected
                and baseline_counts.get("tessdata-best") == expected
            ),
        },
        "candidates": candidates,
    }
    validate_matrix_artifact(artifact, configs)
    return artifact


def _matrix_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    parser.add_argument("--run-matrix", action="store_true")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--report-from", type=Path)
    parser.add_argument("--configs", type=Path, default=DEFAULT_CONFIGS)
    parser.add_argument("--annotations", type=Path, default=DEFAULT_ANNOTATIONS)
    parser.add_argument("--sources", type=Path, default=DEFAULT_SOURCES)
    parser.add_argument("--assets-dir", type=Path, default=DEFAULT_ASSETS)
    parser.add_argument("--baseline-run", type=Path, default=DEFAULT_BASELINE_RUN)
    parser.add_argument(
        "--fileconv", type=Path, default=ROOT / "target" / "release" / "fileconv"
    )
    parser.add_argument("--shim", type=Path, default=DEFAULT_SHIM)
    parser.add_argument(
        "--real-tesseract",
        type=Path,
        default=Path(shutil.which("tesseract") or "/usr/bin/tesseract"),
    )
    parser.add_argument(
        "--system-tessdata",
        type=Path,
        default=Path("/usr/share/tesseract-ocr/5/tessdata"),
    )
    parser.add_argument("--work-dir", type=Path, default=DEFAULT_MATRIX_WORK)
    return parser


def matrix_main(argv: list[str] | None = None) -> int:
    args = _matrix_parser().parse_args(argv)
    if args.report_from:
        if args.output is None:
            raise SystemExit("--output is required with --report-from")
        artifact = json.loads(args.report_from.read_text(encoding="utf-8"))
        validate_matrix_artifact(artifact, load_experiment_configs(args.configs))
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(render_matrix_report(artifact), encoding="utf-8")
        return 0
    if not args.run_matrix or args.output is None:
        raise SystemExit("--run-matrix and --output are required")
    artifact = run_tuning_matrix(args)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps(artifact, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    return 0


if __name__ == "__main__":
    if any(argument in {"--run-matrix", "--report-from"} for argument in sys.argv[1:]):
        raise SystemExit(matrix_main())
    raise SystemExit(shim_main())
