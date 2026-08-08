"""Serial, cache-only Phase A benchmark runner."""

from __future__ import annotations

import argparse
import hashlib
import importlib.metadata
import json
import math
import platform
import select
import subprocess
import sys
import time
from collections.abc import Mapping, Set
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from pathlib import Path, PurePosixPath
from typing import Any, Protocol
from urllib.parse import urlsplit

import psutil
import pypdfium2 as pdfium
from PIL import Image, ImageDraw, ImageFont

from benchmark.candidates import CommandCandidateSpec
from benchmark.corpus import (
    BenchmarkPage,
    generate_reviewed_multicolumn_case as corpus_generate_multicolumn_case,
    inspect_and_render_historical as corpus_render_historical,
    inspect_and_render_official as corpus_render_official,
    load_quantitative_pages as corpus_load_quantitative_pages,
)
from benchmark.render import RenderLimits
from benchmark.metrics import error_counts, reading_order_violations
from benchmark.report import aggregate_records, recompute_and_validate_summary
from corpus.download import CorpusSource, load_sources
from markhand_ocr.render import render_page

ROOT = Path(__file__).resolve().parents[3]
SERVICE_ROOT = ROOT / "bench" / "ocr_cpu_service"
DEFAULT_MANIFEST = SERVICE_ROOT / "corpus" / "sources.json"
DEFAULT_CORPUS = SERVICE_ROOT / ".data" / "corpus"
DEFAULT_WORK = SERVICE_ROOT / ".data" / "benchmark"
OFFICIAL_SOURCE_ID = "official-89-2026-tt-btc"
HISTORICAL_READING_ORDER_ANCHORS = {
    ("wikimedia-dai-nam-1907-804", 4): (
        "NHỜI ĐÀN BÀ",
        "RAO HẸN",
        "TẬP THƠ, PHÚ, CA, RAO",
        "CÁO BẠCH",
        "HIỆN BÁO HOÀN CẦU",
    ),
}
FILECONV_BUILD_COMMAND = (
    "CC=gcc CXX=g++ cargo build --release "
    "-p fileconv-cli --no-default-features"
)


def sanitized_candidate_environment(*, cpu_threads: int) -> dict[str, str]:
    """Return the complete non-secret environment shared by all workers."""
    if cpu_threads <= 0:
        raise ValueError("cpu_threads must be positive")
    threads = str(cpu_threads)
    return {
        "LANG": "C.UTF-8",
        "LC_ALL": "C.UTF-8",
        "OMP_NUM_THREADS": threads,
        "OPENBLAS_NUM_THREADS": threads,
        "PATH": "/usr/local/bin:/usr/bin:/bin",
        "PYTHONNOUSERSITE": "1",
        "PYTHONPATH": "bench/ocr_cpu_service",
    }


@dataclass(frozen=True, slots=True)
class RecognitionMeasurement:
    text: str
    candidate_seconds: float
    resource: dict[str, Any]


@dataclass(frozen=True, slots=True)
class CandidateRunResult:
    """Serializable measurements for one candidate/page execution."""

    candidate_id: str
    success: bool
    record: dict[str, Any]
    metadata: dict[str, Any]


class Candidate(Protocol):
    id: str
    label: str
    metadata: dict[str, Any]

    def recognize(self, page: BenchmarkPage) -> RecognitionMeasurement:
        """Recognize one page with explicit timing and RSS semantics."""


def _json_compatible(value: Any) -> Any:
    """Thaw deeply immutable provenance at the JSON serialization boundary."""
    if value is None or isinstance(value, (str, int, float, bool)):
        return value
    if isinstance(value, Mapping):
        if any(not isinstance(key, str) for key in value):
            raise TypeError("JSON object keys must be strings")
        return {key: _json_compatible(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [_json_compatible(item) for item in value]
    if isinstance(value, Set):
        thawed = [_json_compatible(item) for item in value]
        return sorted(
            thawed,
            key=lambda item: json.dumps(
                item, ensure_ascii=False, sort_keys=True, separators=(",", ":")
            ),
        )
    raise TypeError(f"value is not JSON-compatible: {type(value).__name__}")


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _fileconv_provenance(fileconv: Path) -> dict[str, Any]:
    return {
        "binary_sha256": _sha256(fileconv),
        "build_command": FILECONV_BUILD_COMMAND,
        "build_features": ["no-default-features"],
        "profile": "release",
    }


def _destination(source: CorpusSource) -> str:
    remote = PurePosixPath(urlsplit(source.url).path).name
    return source.id + "".join(Path(remote).suffixes)


def _verified_path(source: CorpusSource, corpus_dir: Path) -> Path:
    path = corpus_dir / _destination(source)
    if not path.is_file():
        raise FileNotFoundError(f"missing validated corpus asset: {source.id}")
    if _sha256(path) != source.sha256:
        raise ValueError(f"corpus checksum mismatch: {source.id}")
    return path


def load_quantitative_pages(
    manifest: Path = DEFAULT_MANIFEST,
    corpus_dir: Path = DEFAULT_CORPUS,
) -> list[BenchmarkPage]:
    """Load only pinned nrl-ai pages with human-verified reference text."""
    sources = load_sources(manifest)
    by_remote_name = {
        PurePosixPath(urlsplit(source.url).path).name: source
        for source in sources
        if source.kind in {"real-scan", "synthetic-scan"}
        and source.classification != "metadata"
    }
    pages: list[BenchmarkPage] = []
    metadata_sources = [
        source
        for source in sources
        if source.id in {"vnocr-real-metadata", "vnocr-synthetic-scan-metadata"}
    ]
    for metadata_source in sorted(metadata_sources, key=lambda item: item.id):
        metadata_path = _verified_path(metadata_source, corpus_dir)
        for line in metadata_path.read_text(encoding="utf-8").splitlines():
            row = json.loads(line)
            source = by_remote_name.get(row["file_name"])
            if source is None:
                raise ValueError(
                    f"metadata references an unpinned image: {row['file_name']}"
                )
            reference = row.get("text")
            if not isinstance(reference, str) or not reference.strip():
                raise ValueError(f"missing human-verified text: {source.id}")
            pages.append(
                BenchmarkPage(
                    source_id=source.id,
                    source_sha256=source.sha256,
                    stratum=source.kind,
                    page_number=1,
                    path=_verified_path(source, corpus_dir),
                    reference=reference,
                )
            )
    pages.sort(key=lambda page: (page.stratum, page.source_id, page.page_number))
    if len(pages) != len(by_remote_name):
        raise ValueError("not every pinned quantitative image has verified text")
    return pages


def deterministic_page_sample(page_count: int) -> tuple[int, ...]:
    if page_count <= 0:
        raise ValueError("page_count must be positive")
    return tuple(sorted({1, (page_count + 1) // 2, page_count}))


def historical_reading_order_anchors(
    source_id: str, page_number: int
) -> tuple[str, ...]:
    """Return the small human-reviewed sequence for a pinned historical page."""
    return HISTORICAL_READING_ORDER_ANCHORS.get(
        (source_id, page_number), ()
    )


def bounded_sample_render_limits(
    *,
    page_width: float,
    page_height: float,
    requested_dpi: int,
    max_pixels: int = 20_000_000,
    max_dimension: int = 5_000,
) -> RenderLimits:
    """Choose the highest integer DPI that fits explicit sample image bounds."""
    if page_width <= 0 or page_height <= 0 or requested_dpi <= 0:
        raise ValueError("page dimensions and DPI must be positive")
    scale = min(
        requested_dpi / 72,
        max_dimension / page_width,
        max_dimension / page_height,
        math.sqrt(max_pixels / (page_width * page_height)),
    )
    dpi = max(1, math.floor(scale * 72))
    while (
        math.ceil(page_width * dpi / 72) > max_dimension
        or math.ceil(page_height * dpi / 72) > max_dimension
        or math.ceil(page_width * dpi / 72)
        * math.ceil(page_height * dpi / 72)
        > max_pixels
    ):
        dpi -= 1
        if dpi <= 0:
            raise ValueError("page cannot fit bounded render dimensions")
    return RenderLimits(
        dpi=dpi,
        max_pixels=max_pixels,
        max_dimension=max_dimension,
    )


def generate_reviewed_multicolumn_case(
    work_dir: Path,
) -> tuple[dict[str, Any], BenchmarkPage]:
    """Create a deterministic two-column page with source-ground-truth order."""
    work_dir.mkdir(parents=True, exist_ok=True)
    output = work_dir / "reviewed-multicolumn-v1.png"
    font_path = Path(
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"
    )
    font = ImageFont.truetype(str(font_path), 44)
    heading_font = ImageFont.truetype(str(font_path), 54)
    anchors = ("L1", "L2", "L3", "R1", "R2", "R3")
    left = (
        ("L1", "Dòng trái thứ nhất"),
        ("L2", "Dòng trái thứ hai"),
        ("L3", "Dòng trái thứ ba"),
    )
    right = (
        ("R1", "Dòng phải thứ nhất"),
        ("R2", "Dòng phải thứ hai"),
        ("R3", "Dòng phải thứ ba"),
    )
    image = Image.new("RGB", (1600, 1000), "white")
    draw = ImageDraw.Draw(image)
    draw.text(
        (100, 70),
        "Trường hợp kiểm tra thứ tự hai cột",
        fill="black",
        font=heading_font,
    )
    for x, rows in ((100, left), (880, right)):
        for index, (anchor, text) in enumerate(rows):
            draw.text(
                (x, 230 + index * 190),
                f"{anchor} {text}",
                fill="black",
                font=font,
            )
    try:
        image.save(output, format="PNG", optimize=False, compress_level=9)
    finally:
        image.close()
    image_sha256 = _sha256(output)
    metadata = {
        "source_id": "reviewed-multicolumn-v1",
        "classification": "synthetic-scan",
        "layout": "two-column-column-major",
        "ground_truth": "deterministic-source",
        "review_status": "reviewed-fixture-contract",
        "expected_anchors": len(anchors),
        "page_number": 1,
        "expected_sequence": list(anchors),
        "image_sha256": image_sha256,
        "font_sha256": _sha256(font_path),
        "generator": "Pillow fixed canvas, coordinates, font, and PNG settings",
    }
    return (
        metadata,
        BenchmarkPage(
            source_id="reviewed-multicolumn-v1",
            source_sha256=image_sha256,
            stratum="reviewed-multicolumn",
            page_number=1,
            path=output,
            reference=None,
            gate_included=False,
            reading_order_anchors=anchors,
        ),
    )


def inspect_and_render_official(
    source: CorpusSource,
    corpus_dir: Path,
    work_dir: Path,
    *,
    dpi: int = 200,
    benchmark_stratum: str = "mixed",
) -> tuple[dict[str, Any], list[BenchmarkPage]]:
    """Inspect every PDF page, then render one bounded deterministic sample."""
    path = _verified_path(source, corpus_dir)
    work_dir.mkdir(parents=True, exist_ok=True)
    document = pdfium.PdfDocument(path)
    try:
        page_count = len(document)
        text_pages = 0
        image_pages = 0
        for index in range(page_count):
            page = document[index]
            try:
                text_page = page.get_textpage()
                try:
                    if text_page.get_text_range().strip():
                        text_pages += 1
                finally:
                    text_page.close()
                if any(
                    True
                    for _ in page.get_objects(
                        filter=(pdfium.raw.FPDF_PAGEOBJ_IMAGE,)
                    )
                ):
                    image_pages += 1
            finally:
                page.close()
        classification = (
            "mixed"
            if text_pages and image_pages
            else "native"
            if text_pages
            else "scan"
            if image_pages
            else "unknown"
        )
        sampled = deterministic_page_sample(page_count)
        pages: list[BenchmarkPage] = []
        sampled_page_classification: dict[str, str] = {}
        sampled_page_dpi: dict[str, int] = {}
        for page_number in sampled:
            output = work_dir / f"{source.id}-p{page_number}.png"
            source_page = document[page_number - 1]
            try:
                text_page = source_page.get_textpage()
                try:
                    has_text = bool(text_page.get_text_range().strip())
                finally:
                    text_page.close()
                has_image = any(
                    True
                    for _ in source_page.get_objects(
                        filter=(pdfium.raw.FPDF_PAGEOBJ_IMAGE,)
                    )
                )
                sampled_page_classification[str(page_number)] = (
                    "mixed"
                    if has_text and has_image
                    else "native"
                    if has_text
                    else "scan"
                    if has_image
                    else "empty"
                )
                width, height = source_page.get_size()
                limits = bounded_sample_render_limits(
                    page_width=width,
                    page_height=height,
                    requested_dpi=dpi,
                )
                sampled_page_dpi[str(page_number)] = limits.dpi
                image = render_page(
                    source_page,
                    limits,
                )
                try:
                    image.save(output, format="PNG")
                finally:
                    image.close()
            finally:
                source_page.close()
            pages.append(
                BenchmarkPage(
                    source_id=source.id,
                    source_sha256=source.sha256,
                    stratum=benchmark_stratum,
                    page_number=page_number,
                    path=output,
                    reference=None,
                    gate_included=False,
                    reading_order_anchors=historical_reading_order_anchors(
                        source.id, page_number
                    ),
                )
            )
    finally:
        document.close()
    return (
        {
            "source_id": source.id,
            "source_sha256": source.sha256,
            "classification": classification,
            "manifest_classification": source.classification,
            "classification_mismatch": source.classification != classification,
            "classification_evidence": {
                "pages": page_count,
                "text_pages": text_pages,
                "image_pages": image_pages,
            },
            "sampled_pages": list(sampled),
            "sampled_page_classification": sampled_page_classification,
            "requested_render_dpi": dpi,
            "sampled_page_render_dpi": sampled_page_dpi,
            "rendered_page_sha256": {
                str(page.page_number): _sha256(page.path) for page in pages
            },
        },
        pages,
    )


def inspect_and_render_historical(
    sources: list[CorpusSource],
    corpus_dir: Path,
    work_dir: Path,
    *,
    dpi: int = 200,
) -> tuple[list[dict[str, Any]], list[BenchmarkPage]]:
    """Render bounded manifest-pinned historical samples as qualitative evidence."""
    evidence: list[dict[str, Any]] = []
    pages: list[BenchmarkPage] = []
    for source in sorted(sources, key=lambda item: item.id):
        source_evidence, source_pages = inspect_and_render_official(
            source,
            corpus_dir,
            work_dir,
            dpi=dpi,
            benchmark_stratum="historical-scan",
        )
        source_evidence["transcription"] = "none-trustworthy-available"
        source_evidence["evidence_mode"] = "qualitative-only"
        reviewed_pages = [
            page for page in source_pages if page.reading_order_anchors
        ]
        if reviewed_pages:
            if len(reviewed_pages) != 1:
                raise ValueError(
                    f"{source.id}: expected one reviewed reading-order page"
                )
            reviewed_page = reviewed_pages[0]
            source_evidence["reading_order_review"] = {
                "page_number": reviewed_page.page_number,
                "review_status": "human-reviewed-short-anchors",
                "expected_anchors": len(
                    reviewed_page.reading_order_anchors
                ),
                "expected_sequence": list(
                    reviewed_page.reading_order_anchors
                ),
                "matching": (
                    "accent/punctuation folded; <=25% character edits"
                ),
            }
        evidence.append(source_evidence)
        pages.extend(source_pages)
    return evidence, pages


def _process_tree_rss(process: psutil.Process) -> int:
    processes = [process]
    try:
        processes.extend(process.children(recursive=True))
    except (psutil.Error, ProcessLookupError):
        pass
    total = 0
    for item in processes:
        try:
            total += item.memory_info().rss
        except (psutil.Error, ProcessLookupError):
            pass
    return total


def _read_event_with_process_tree_rss(
    process: subprocess.Popen[str],
    *,
    timeout_seconds: float,
    sample_interval_seconds: float = 0.01,
) -> tuple[dict[str, Any], dict[str, Any]]:
    """Read one worker event while sampling the complete descendant tree."""
    if process.stdout is None:
        raise ValueError("worker stdout must be piped")
    started = time.perf_counter()
    monitored = psutil.Process(process.pid)
    peak_rss = 0
    sample_count = 0
    while True:
        peak_rss = max(peak_rss, _process_tree_rss(monitored))
        sample_count += 1
        elapsed = time.perf_counter() - started
        if elapsed > timeout_seconds:
            process.kill()
            process.wait()
            raise TimeoutError("candidate worker event timeout")
        readable, _, _ = select.select(
            [process.stdout], [], [], sample_interval_seconds
        )
        if readable:
            line = process.stdout.readline()
            if not line:
                raise RuntimeError("candidate worker exited without an event")
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                continue
            if not isinstance(event, dict) or "event" not in event:
                continue
            return (
                event,
                {
                    "method": "sampled_process_tree_rss",
                    "peak_rss_bytes": peak_rss,
                    "sample_count": sample_count,
                    "sample_interval_seconds": sample_interval_seconds,
                    "wall_seconds": time.perf_counter() - started,
                },
            )
        if process.poll() is not None:
            raise RuntimeError("candidate worker exited without an event")


class IsolatedCandidateWorker:
    """One candidate in its own long-lived, consistently monitored process."""

    def __init__(
        self,
        *,
        candidate_id: str,
        label: str,
        command: list[str],
        environment: dict[str, str],
        timeout_seconds: float,
        command_description: str = "isolated candidate worker",
        provenance: dict[str, Any] | None = None,
    ) -> None:
        self.id = candidate_id
        self.label = label
        self._timeout_seconds = timeout_seconds
        process_invoked = time.perf_counter()
        self._process = subprocess.Popen(
            command,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            bufsize=1,
            env=environment,
        )
        event, resource = _read_event_with_process_tree_rss(
            self._process, timeout_seconds=timeout_seconds
        )
        cold_wall_seconds = time.perf_counter() - process_invoked
        if event.get("event") != "ready":
            self.close()
            raise RuntimeError("candidate worker did not become ready")
        self.metadata = {
            "worker_command": command_description,
            "environment": dict(environment),
            "cold_initialization": {
                "candidate_seconds": event["candidate_seconds"],
                "wall_seconds": cold_wall_seconds,
                "timing_scope": "worker_process_invocation_to_ready",
                "process_startup_included": True,
                "rss_measurement": resource,
            },
            **_json_compatible(provenance or {}),
        }

    def recognize(self, page: BenchmarkPage) -> RecognitionMeasurement:
        if self._process.stdin is None:
            raise RuntimeError("candidate worker stdin is unavailable")
        self._process.stdin.write(
            json.dumps(
                {
                    "event": "recognize",
                    "path": str(page.path),
                    "page_number": page.page_number,
                }
            )
            + "\n"
        )
        self._process.stdin.flush()
        event, resource = _read_event_with_process_tree_rss(
            self._process, timeout_seconds=self._timeout_seconds
        )
        if event.get("event") != "result":
            raise RuntimeError("candidate worker returned an invalid event")
        return RecognitionMeasurement(
            text=event["text"],
            candidate_seconds=event["candidate_seconds"],
            resource=resource,
        )

    def close(self) -> None:
        if self._process.poll() is not None:
            return
        if self._process.stdin is not None:
            try:
                self._process.stdin.write('{"event":"shutdown"}\n')
                self._process.stdin.flush()
            except BrokenPipeError:
                pass
        try:
            self._process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self._process.kill()
            self._process.wait()


def _isolated_worker(
    spec: CommandCandidateSpec, *, timeout_seconds: float
) -> IsolatedCandidateWorker:
    return IsolatedCandidateWorker(
        candidate_id=spec.id,
        label=spec.label,
        command=[
            sys.executable,
            "-m",
            "benchmark.worker",
            "--argv-json",
            json.dumps(spec.argv, ensure_ascii=False),
        ],
        environment=dict(spec.environment),
        timeout_seconds=timeout_seconds,
        command_description="python -m benchmark.worker --argv-json <candidate argv>",
        provenance=_json_compatible(spec.provenance),
    )


def run_candidate(
    spec: CommandCandidateSpec,
    page: BenchmarkPage,
    *,
    timeout_seconds: float,
    max_rss_bytes: int,
) -> CandidateRunResult:
    """Run one arbitrary candidate without retaining recognized text."""
    candidate = _isolated_worker(spec, timeout_seconds=timeout_seconds)
    try:
        result = _run_candidate(candidate, [page], max_rss_bytes=max_rss_bytes)
    finally:
        candidate.close()
    record = result["pages"][0]
    return CandidateRunResult(
        candidate_id=spec.id,
        success=bool(record["success"]),
        record=record,
        metadata=result["metadata"],
    )


def _run_candidate(
    candidate: Candidate,
    pages: list[BenchmarkPage],
    *,
    max_rss_bytes: int,
) -> dict[str, Any]:
    records: list[dict[str, Any]] = []
    for page in pages:
        record: dict[str, Any] = {
            "source_id": page.source_id,
            "source_sha256": page.source_sha256,
            "stratum": page.stratum,
            "gate_included": page.gate_included,
            "page_number": page.page_number,
            "success": False,
            "error_kind": None,
            "elapsed_seconds": 0.0,
            "timing_scope": "warm_worker_request_wall",
            "process_startup_included": False,
            "peak_rss_bytes": 0,
            "resource_limit_violation": False,
        }
        try:
            measurement = candidate.recognize(page)
            elapsed = measurement.resource["wall_seconds"]
            peak_rss = measurement.resource["peak_rss_bytes"]
            record.update(
                success=True,
                elapsed_seconds=elapsed,
                candidate_seconds=measurement.candidate_seconds,
                peak_rss_bytes=peak_rss,
                rss_measurement=measurement.resource,
                resource_limit_violation=peak_rss > max_rss_bytes,
            )
            if page.reference is not None:
                counts = error_counts(page.reference, measurement.text)
                record.update(
                    character_edits=counts.character_edits,
                    reference_characters=counts.reference_characters,
                    word_edits=counts.word_edits,
                    reference_words=counts.reference_words,
                    cer=(
                        counts.character_edits / counts.reference_characters
                        if counts.reference_characters
                        else (0.0 if not measurement.text else 1.0)
                    ),
                    wer=(
                        counts.word_edits / counts.reference_words
                        if counts.reference_words
                        else (0.0 if not measurement.text else 1.0)
                    ),
                )
            if page.reading_order_anchors:
                record["reading_order"] = asdict(
                    reading_order_violations(
                        page.reading_order_anchors, measurement.text
                    )
                )
        except TimeoutError:
            record["error_kind"] = "timeout"
        except Exception:
            record["error_kind"] = "candidate_error"
        records.append(record)
        print(
            f"[{candidate.id}] {page.source_id} p{page.page_number}: "
            f"{'ok' if record['success'] else record['error_kind']}",
            file=sys.stderr,
            flush=True,
        )
    quantitative = [
        record
        for record in records
        if record["gate_included"]
    ]
    strata = {
        stratum: aggregate_records(
            [record for record in quantitative if record["stratum"] == stratum]
        )
        for stratum in sorted({record["stratum"] for record in quantitative})
    }
    return {
        "id": candidate.id,
        "label": candidate.label,
        "metadata": candidate.metadata,
        "aggregate": aggregate_records(quantitative),
        "strata": strata,
        "pages": records,
    }


def _command_version(command: list[str]) -> str:
    result = subprocess.run(
        command, capture_output=True, text=True, check=True, timeout=30
    )
    return (result.stdout or result.stderr).splitlines()[0].strip()


def run_benchmark(args: argparse.Namespace) -> dict[str, Any]:
    manifest = args.manifest.resolve()
    corpus_dir = args.corpus_dir.resolve()
    sources = load_sources(manifest)
    quantitative_pages = corpus_load_quantitative_pages(manifest, corpus_dir)
    official_source = next(
        source for source in sources if source.id == OFFICIAL_SOURCE_ID
    )
    official, official_pages = corpus_render_official(
        official_source, corpus_dir, args.work_dir.resolve()
    )
    historical, historical_pages = corpus_render_historical(
        [source for source in sources if source.kind == "wikimedia-scan"],
        corpus_dir,
        args.work_dir.resolve(),
    )
    multicolumn, multicolumn_page = corpus_generate_multicolumn_case(
        args.work_dir.resolve()
    )
    pages = (
        quantitative_pages
        + official_pages
        + historical_pages
        + [multicolumn_page]
    )
    fileconv = args.fileconv.resolve()
    system_tessdata = args.system_tessdata.resolve()
    best_tessdata = args.best_tessdata.resolve()
    environment = sanitized_candidate_environment(
        cpu_threads=args.cpu_threads
    )
    fileconv_build = _fileconv_provenance(fileconv)
    candidates = [
        CommandCandidateSpec(
            id="markhand-default",
            label="Markhand default",
            argv=(
                str(fileconv),
                "one",
                "{input}",
                "--lang",
                "vie+eng",
            ),
            environment={
                **environment,
                "FILECONV_TESSDATA": str(system_tessdata),
            },
            provenance={
                "fileconv_build": fileconv_build,
                "invocation": "fileconv one <identical-page.png> --lang vie+eng",
                "timing_note": (
                    "warm timing includes a fresh fileconv/Tesseract subprocess "
                    "spawn, execution, and output collection for every page"
                ),
                "tessdata_role": "system-default",
                "tessdata_sha256": {
                    language: _sha256(
                        system_tessdata / f"{language}.traineddata"
                    )
                    for language in ("vie", "eng")
                },
            },
        ),
        CommandCandidateSpec(
            id="markhand-tessdata-best",
            label="Markhand tessdata_best",
            argv=(
                str(fileconv),
                "one",
                "{input}",
                "--lang",
                "vie+eng",
            ),
            environment={
                **environment,
                "FILECONV_TESSDATA": str(best_tessdata),
            },
            provenance={
                "fileconv_build": fileconv_build,
                "invocation": "fileconv one <identical-page.png> --lang vie+eng",
                "timing_note": (
                    "warm timing includes a fresh fileconv/Tesseract subprocess "
                    "spawn, execution, and output collection for every page"
                ),
                "tessdata_role": "best",
                "tessdata_sha256": {
                    language: _sha256(
                        best_tessdata / f"{language}.traineddata"
                    )
                    for language in ("vie", "eng")
                },
            },
        ),
    ]
    candidate_results: list[dict[str, Any]] = []
    for spec in candidates:
        candidate = _isolated_worker(
            CommandCandidateSpec(
                id=spec.id,
                label=spec.label,
                argv=spec.argv,
                environment=spec.environment,
                provenance={
                    **dict(spec.provenance),
                    "measurement_semantics": {
                        "cold_initialization": (
                            "worker process start through candidate-ready event"
                        ),
                        "warm_page_latency": (
                            "parent wall time from request flush through result "
                            "event; worker remains initialized"
                        ),
                        "rss": (
                            "10 ms sampled sum of worker and descendant RSS during "
                            "the measured interval"
                        ),
                    },
                },
            ),
            timeout_seconds=args.timeout_seconds,
        )
        try:
            candidate_results.append(
                _run_candidate(
                    candidate, pages, max_rss_bytes=args.max_rss_bytes
                )
            )
        finally:
            candidate.close()
    memory = psutil.virtual_memory()
    summary = {
        "schema_version": 1,
        "generated_at_utc": datetime.now(timezone.utc)
        .isoformat(timespec="seconds")
        .replace("+00:00", "Z"),
        "run": {
            "commit": subprocess.run(
                ["git", "rev-parse", "HEAD"],
                cwd=ROOT,
                capture_output=True,
                text=True,
                check=True,
            ).stdout.strip(),
            "host": {
                "platform": platform.platform(),
                "architecture": platform.machine(),
                "logical_cpus": psutil.cpu_count(logical=True),
                "memory_bytes": memory.total,
                "max_rss_bytes": args.max_rss_bytes,
            },
            "versions": {
                "cargo": _command_version(["cargo", "--version"]),
                "pypdfium2": importlib.metadata.version("pypdfium2"),
                "python": platform.python_version(),
                "tesseract": _command_version(["tesseract", "--version"]),
            },
        },
        "corpus": {
            "manifest_sha256": _sha256(manifest),
            "quantitative_pages": len(quantitative_pages),
            "representativeness": {
                "bounded_sample": True,
                "population_estimate": False,
                "confidence_interval_claimed": False,
                "documents_per_source": 1,
            },
            "strata": {
                stratum: sum(
                    page.stratum == stratum for page in quantitative_pages
                )
                for stratum in sorted(
                    {page.stratum for page in quantitative_pages}
                )
            },
            "official_sample": official,
            "historical_samples": historical,
            "reading_order_cases": [
                multicolumn,
                *[
                    {
                        "source_id": sample["source_id"],
                        "classification": sample["classification"],
                        "ground_truth": sample["reading_order_review"][
                            "review_status"
                        ],
                        **sample["reading_order_review"],
                    }
                    for sample in historical
                    if "reading_order_review" in sample
                ],
            ],
        },
        "candidates": candidate_results,
    }
    return recompute_and_validate_summary(_json_compatible(summary))


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--corpus-dir", type=Path, default=DEFAULT_CORPUS)
    parser.add_argument("--work-dir", type=Path, default=DEFAULT_WORK)
    parser.add_argument(
        "--fileconv", type=Path, default=ROOT / "target" / "release" / "fileconv"
    )
    parser.add_argument(
        "--system-tessdata",
        type=Path,
        default=Path("/usr/share/tesseract-ocr/5/tessdata"),
    )
    parser.add_argument(
        "--best-tessdata", type=Path, default=ROOT / "tessdata_best"
    )
    parser.add_argument("--timeout-seconds", type=float, default=180.0)
    parser.add_argument(
        "--cpu-threads", type=int, default=psutil.cpu_count(logical=True) or 1
    )
    parser.add_argument(
        "--max-rss-bytes", type=int, default=4 * 1024 * 1024 * 1024
    )
    return parser


def main() -> None:
    args = _parser().parse_args()
    summary = run_benchmark(args)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps(
            _json_compatible(summary),
            ensure_ascii=False,
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )


if __name__ == "__main__":
    main()
