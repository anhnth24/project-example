"""Serial, cache-only Phase A benchmark runner."""

from __future__ import annotations

import argparse
import hashlib
import importlib.metadata
import json
import platform
import select
import subprocess
import sys
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path, PurePosixPath
from typing import Any, Protocol
from urllib.parse import urlsplit

import psutil
import pymupdf

from benchmark.metrics import error_counts
from benchmark.report import aggregate_records, recompute_and_validate_summary
from corpus.download import CorpusSource, load_sources

ROOT = Path(__file__).resolve().parents[3]
SERVICE_ROOT = ROOT / "bench" / "ocr_cpu_service"
DEFAULT_MANIFEST = SERVICE_ROOT / "corpus" / "sources.json"
DEFAULT_CORPUS = SERVICE_ROOT / ".data" / "corpus"
DEFAULT_WORK = SERVICE_ROOT / ".data" / "benchmark"
OFFICIAL_SOURCE_ID = "official-89-2026-tt-btc"
MODEL_ASSETS = ("inference.json", "inference.yml", "inference.pdiparams")
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
        "PADDLE_PDX_DISABLE_MODEL_SOURCE_CHECK": "True",
        "PYTHONNOUSERSITE": "1",
        "PYTHONPATH": "bench/ocr_cpu_service",
    }


@dataclass(frozen=True, slots=True)
class BenchmarkPage:
    source_id: str
    source_sha256: str
    stratum: str
    page_number: int
    path: Path
    reference: str | None
    gate_included: bool = True


@dataclass(frozen=True, slots=True)
class RecognitionMeasurement:
    text: str
    candidate_seconds: float
    resource: dict[str, Any]


class Candidate(Protocol):
    id: str
    label: str
    metadata: dict[str, Any]

    def recognize(self, page: BenchmarkPage) -> RecognitionMeasurement:
        """Recognize one page with explicit timing and RSS semantics."""


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


def inspect_and_render_official(
    source: CorpusSource,
    corpus_dir: Path,
    work_dir: Path,
    *,
    dpi: int = 200,
) -> tuple[dict[str, Any], list[BenchmarkPage]]:
    """Inspect every official page, then render one bounded deterministic sample."""
    path = _verified_path(source, corpus_dir)
    work_dir.mkdir(parents=True, exist_ok=True)
    document = pymupdf.open(path)
    try:
        page_count = document.page_count
        text_pages = 0
        image_pages = 0
        for page in document:
            if page.get_text().strip():
                text_pages += 1
            if page.get_images(full=True):
                image_pages += 1
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
        matrix = pymupdf.Matrix(dpi / 72, dpi / 72)
        for page_number in sampled:
            output = work_dir / f"official-89-2026-tt-btc-p{page_number}.png"
            source_page = document.load_page(page_number - 1)
            has_text = bool(source_page.get_text().strip())
            has_image = bool(source_page.get_images(full=True))
            sampled_page_classification[str(page_number)] = (
                "mixed"
                if has_text and has_image
                else "native"
                if has_text
                else "scan"
                if has_image
                else "empty"
            )
            pixmap = source_page.get_pixmap(
                matrix=matrix, colorspace=pymupdf.csRGB, alpha=False
            )
            pixmap.save(output)
            pages.append(
                BenchmarkPage(
                    source_id=source.id,
                    source_sha256=source.sha256,
                    stratum="mixed",
                    page_number=page_number,
                    path=output,
                    reference=None,
                    gate_included=False,
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
            "render_dpi": dpi,
            "rendered_page_sha256": {
                str(page.page_number): _sha256(page.path) for page in pages
            },
        },
        pages,
    )


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
            "environment": environment,
            "cold_initialization": {
                "candidate_seconds": event["candidate_seconds"],
                "wall_seconds": cold_wall_seconds,
                "timing_scope": "worker_process_invocation_to_ready",
                "process_startup_included": True,
                "rss_measurement": resource,
            },
            **(provenance or {}),
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


def _version(package: str) -> str:
    return importlib.metadata.version(package)


def _command_version(command: list[str]) -> str:
    result = subprocess.run(
        command, capture_output=True, text=True, check=True, timeout=30
    )
    return (result.stdout or result.stderr).splitlines()[0].strip()


def run_benchmark(args: argparse.Namespace) -> dict[str, Any]:
    manifest = args.manifest.resolve()
    corpus_dir = args.corpus_dir.resolve()
    sources = load_sources(manifest)
    quantitative_pages = load_quantitative_pages(manifest, corpus_dir)
    official_source = next(
        source for source in sources if source.id == OFFICIAL_SOURCE_ID
    )
    official, official_pages = inspect_and_render_official(
        official_source, corpus_dir, args.work_dir.resolve()
    )
    pages = quantitative_pages + official_pages
    fileconv = args.fileconv.resolve()
    system_tessdata = args.system_tessdata.resolve()
    best_tessdata = args.best_tessdata.resolve()
    detection_dir = args.paddle_detection_dir.resolve()
    recognition_dir = args.paddle_recognition_dir.resolve()
    environment = sanitized_candidate_environment(
        cpu_threads=args.cpu_threads
    )
    worker_base = [sys.executable, "-m", "benchmark.worker"]
    fileconv_build = _fileconv_provenance(fileconv)
    configurations = [
        {
            "id": "markhand-default",
            "label": "Markhand default",
            "command": worker_base
            + [
                "--candidate",
                "markhand-default",
                "--fileconv",
                str(fileconv),
                "--tessdata",
                str(system_tessdata),
            ],
            "provenance": {
                "fileconv_build": fileconv_build,
                "invocation": "fileconv one <identical-page.png> --lang vie+eng",
                "tessdata_role": "system-default",
                "tessdata_sha256": {
                    language: _sha256(
                        system_tessdata / f"{language}.traineddata"
                    )
                    for language in ("vie", "eng")
                },
            },
        },
        {
            "id": "markhand-tessdata-best",
            "label": "Markhand tessdata_best",
            "command": worker_base
            + [
                "--candidate",
                "markhand-tessdata-best",
                "--fileconv",
                str(fileconv),
                "--tessdata",
                str(best_tessdata),
            ],
            "provenance": {
                "fileconv_build": fileconv_build,
                "invocation": "fileconv one <identical-page.png> --lang vie+eng",
                "tessdata_role": "best",
                "tessdata_sha256": {
                    language: _sha256(
                        best_tessdata / f"{language}.traineddata"
                    )
                    for language in ("vie", "eng")
                },
            },
        },
        {
            "id": "pp-ocrv6",
            "label": "PP-OCRv6",
            "command": worker_base
            + [
                "--candidate",
                "pp-ocrv6",
                "--detection-dir",
                str(detection_dir),
                "--recognition-dir",
                str(recognition_dir),
            ],
            "provenance": {
                "invocation": "cached CPU public API over identical page image",
                "model_asset_sha256": {
                    role: {
                        asset: _sha256(directory / asset)
                        for asset in MODEL_ASSETS
                    }
                    for role, directory in (
                        ("detection", detection_dir),
                        ("recognition", recognition_dir),
                    )
                },
            },
        },
    ]
    candidate_results: list[dict[str, Any]] = []
    for configuration in configurations:
        candidate = IsolatedCandidateWorker(
            candidate_id=configuration["id"],
            label=configuration["label"],
            command=configuration["command"],
            environment=environment,
            timeout_seconds=args.timeout_seconds,
            command_description=(
                "python -m benchmark.worker --candidate "
                f"{configuration['id']} <role-specific local assets>"
            ),
            provenance={
                **configuration["provenance"],
                "measurement_semantics": {
                    "cold_initialization": (
                        "worker process start through candidate-ready event"
                    ),
                    "warm_page_latency": (
                        "parent wall time from request flush through result event; "
                        "worker remains initialized"
                    ),
                    "rss": (
                        "10 ms sampled sum of worker and descendant RSS during "
                        "the measured interval"
                    ),
                },
            },
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
                "paddleocr": _version("paddleocr"),
                "paddlepaddle": _version("paddlepaddle"),
                "paddlex": _version("paddlex"),
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
        },
        "candidates": candidate_results,
    }
    return recompute_and_validate_summary(summary)


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
    parser.add_argument(
        "--paddle-detection-dir",
        type=Path,
        default=SERVICE_ROOT / ".data" / "models" / "detection",
    )
    parser.add_argument(
        "--paddle-recognition-dir",
        type=Path,
        default=SERVICE_ROOT / ".data" / "models" / "recognition",
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
        json.dumps(summary, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )


if __name__ == "__main__":
    main()
