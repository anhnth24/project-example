"""Serial, cache-only Phase A benchmark runner."""

from __future__ import annotations

import argparse
import hashlib
import importlib.metadata
import json
import os
import platform
import statistics
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
from PIL import Image

from benchmark.metrics import error_counts
from benchmark.report import evaluate_gate
from corpus.download import CorpusSource, load_sources
from markhand_ocr.ordering import order_spans
from markhand_ocr.paddle_backend import PaddleOcrBackend

ROOT = Path(__file__).resolve().parents[3]
SERVICE_ROOT = ROOT / "bench" / "ocr_cpu_service"
DEFAULT_MANIFEST = SERVICE_ROOT / "corpus" / "sources.json"
DEFAULT_CORPUS = SERVICE_ROOT / ".data" / "corpus"
DEFAULT_WORK = SERVICE_ROOT / ".data" / "benchmark"
OFFICIAL_SOURCE_ID = "official-89-2026-tt-btc"
MODEL_ASSETS = ("inference.json", "inference.yml", "inference.pdiparams")


@dataclass(frozen=True, slots=True)
class BenchmarkPage:
    source_id: str
    source_sha256: str
    stratum: str
    page_number: int
    path: Path
    reference: str | None


class Candidate(Protocol):
    id: str
    label: str
    metadata: dict[str, Any]

    def recognize(self, page: BenchmarkPage) -> tuple[str, float, int]:
        """Return recognized text, elapsed seconds, and peak RSS bytes."""


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


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
        matrix = pymupdf.Matrix(dpi / 72, dpi / 72)
        for page_number in sampled:
            output = work_dir / f"official-89-2026-tt-btc-p{page_number}.png"
            pixmap = document.load_page(page_number - 1).get_pixmap(
                matrix=matrix, colorspace=pymupdf.csRGB, alpha=False
            )
            pixmap.save(output)
            pages.append(
                BenchmarkPage(
                    source_id=source.id,
                    source_sha256=source.sha256,
                    stratum="official-government",
                    page_number=page_number,
                    path=output,
                    reference=None,
                )
            )
    finally:
        document.close()
    return (
        {
            "source_id": source.id,
            "source_sha256": source.sha256,
            "classification": classification,
            "classification_evidence": {
                "pages": page_count,
                "text_pages": text_pages,
                "image_pages": image_pages,
            },
            "sampled_pages": list(sampled),
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


class MarkhandCandidate:
    def __init__(
        self,
        *,
        candidate_id: str,
        label: str,
        fileconv: Path,
        tessdata: Path,
        timeout_seconds: float,
    ) -> None:
        self.id = candidate_id
        self.label = label
        self._fileconv = fileconv
        self._tessdata = tessdata
        self._timeout_seconds = timeout_seconds
        self.metadata = {
            "invocation": "fileconv one <identical-page.png> --lang vie+eng",
            "tessdata_role": (
                "system-default" if candidate_id == "markhand-default" else "best"
            ),
            "tessdata_sha256": {
                language: _sha256(tessdata / f"{language}.traineddata")
                for language in ("vie", "eng")
            },
        }

    def recognize(self, page: BenchmarkPage) -> tuple[str, float, int]:
        environment = os.environ.copy()
        environment["FILECONV_TESSDATA"] = str(self._tessdata)
        started = time.perf_counter()
        process = subprocess.Popen(
            [
                str(self._fileconv),
                "one",
                str(page.path),
                "--lang",
                "vie+eng",
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env=environment,
        )
        ps_process = psutil.Process(process.pid)
        peak_rss = 0
        while process.poll() is None:
            peak_rss = max(peak_rss, _process_tree_rss(ps_process))
            if time.perf_counter() - started > self._timeout_seconds:
                process.kill()
                process.communicate()
                raise TimeoutError("candidate page timeout")
            time.sleep(0.01)
        stdout, _stderr = process.communicate()
        elapsed = time.perf_counter() - started
        if process.returncode:
            raise RuntimeError("candidate process failed")
        return stdout, elapsed, peak_rss


class PaddleCandidate:
    id = "pp-ocrv6"
    label = "PP-OCRv6"

    def __init__(self, detection_dir: Path, recognition_dir: Path) -> None:
        started = time.perf_counter()
        self._backend = PaddleOcrBackend(
            text_detection_model_dir=detection_dir,
            text_recognition_model_dir=recognition_dir,
        )
        self.initialization_seconds = time.perf_counter() - started
        self.metadata = {
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
            "initialization_seconds": self.initialization_seconds,
        }

    def recognize(self, page: BenchmarkPage) -> tuple[str, float, int]:
        process = psutil.Process()
        started = time.perf_counter()
        peak_rss = process.memory_info().rss
        with Image.open(page.path) as image:
            spans = self._backend.recognize(image.convert("RGB"))
            peak_rss = max(peak_rss, process.memory_info().rss)
            ordered = order_spans(spans, page_width=image.width)
        elapsed = time.perf_counter() - started
        return "\n".join(span.text for span in ordered), elapsed, peak_rss


def _percentile(values: list[float], percentile: float) -> float:
    ordered = sorted(values)
    if not ordered:
        return 0.0
    position = (len(ordered) - 1) * percentile
    lower = int(position)
    upper = min(lower + 1, len(ordered) - 1)
    fraction = position - lower
    return ordered[lower] + (ordered[upper] - ordered[lower]) * fraction


def aggregate_records(records: list[dict[str, Any]]) -> dict[str, Any]:
    successful = [record for record in records if record["success"]]
    character_total = sum(
        record.get("reference_characters", 0) for record in successful
    )
    word_total = sum(record.get("reference_words", 0) for record in successful)
    latencies = [record["elapsed_seconds"] for record in successful]
    return {
        "pages": len(records),
        "cer": (
            sum(record.get("character_edits", 0) for record in successful)
            / character_total
            if character_total
            else 0.0
        ),
        "wer": (
            sum(record.get("word_edits", 0) for record in successful)
            / word_total
            if word_total
            else 0.0
        ),
        "median_seconds_per_page": (
            statistics.median(latencies) if latencies else 0.0
        ),
        "p95_seconds_per_page": _percentile(latencies, 0.95),
        "peak_rss_bytes": max(
            (record.get("peak_rss_bytes", 0) for record in records), default=0
        ),
        "failures": len(records) - len(successful),
    }


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
            "page_number": page.page_number,
            "success": False,
            "error_kind": None,
            "elapsed_seconds": 0.0,
            "peak_rss_bytes": 0,
            "resource_limit_violation": False,
        }
        try:
            hypothesis, elapsed, peak_rss = candidate.recognize(page)
            record.update(
                success=True,
                elapsed_seconds=elapsed,
                peak_rss_bytes=peak_rss,
                resource_limit_violation=peak_rss > max_rss_bytes,
            )
            if page.reference is not None:
                counts = error_counts(page.reference, hypothesis)
                record.update(
                    character_edits=counts.character_edits,
                    reference_characters=counts.reference_characters,
                    word_edits=counts.word_edits,
                    reference_words=counts.reference_words,
                    cer=(
                        counts.character_edits / counts.reference_characters
                        if counts.reference_characters
                        else (0.0 if not hypothesis else 1.0)
                    ),
                    wer=(
                        counts.word_edits / counts.reference_words
                        if counts.reference_words
                        else (0.0 if not hypothesis else 1.0)
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
        if record["stratum"] in {"real-scan", "synthetic-scan"}
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

    candidates: list[Candidate] = [
        MarkhandCandidate(
            candidate_id="markhand-default",
            label="Markhand default",
            fileconv=args.fileconv.resolve(),
            tessdata=args.system_tessdata.resolve(),
            timeout_seconds=args.timeout_seconds,
        ),
        MarkhandCandidate(
            candidate_id="markhand-tessdata-best",
            label="Markhand tessdata_best",
            fileconv=args.fileconv.resolve(),
            tessdata=args.best_tessdata.resolve(),
            timeout_seconds=args.timeout_seconds,
        ),
        PaddleCandidate(
            args.paddle_detection_dir.resolve(),
            args.paddle_recognition_dir.resolve(),
        ),
    ]
    candidate_results = [
        _run_candidate(candidate, pages, max_rss_bytes=args.max_rss_bytes)
        for candidate in candidates
    ]
    by_id = {candidate["id"]: candidate for candidate in candidate_results}
    default = by_id["markhand-default"]
    best = by_id["markhand-tessdata-best"]
    paddle = by_id["pp-ocrv6"]
    baseline_strata = {
        stratum: min(
            default["strata"][stratum]["cer"],
            best["strata"][stratum]["cer"],
        )
        for stratum in default["strata"]
    }
    failures = sum(
        1
        for candidate in candidate_results
        for record in candidate["pages"]
        if not record["success"]
    )
    resource_violations = sum(
        1
        for candidate in candidate_results
        for record in candidate["pages"]
        if record["resource_limit_violation"]
    )
    gate = evaluate_gate(
        default["strata"]["real-scan"]["cer"],
        best["strata"]["real-scan"]["cer"],
        paddle["strata"]["real-scan"]["cer"],
        baseline_cer_by_stratum=baseline_strata,
        paddle_cer_by_stratum={
            stratum: metrics["cer"]
            for stratum, metrics in paddle["strata"].items()
        },
        failures=failures,
        resource_limit_violations=resource_violations,
    )
    memory = psutil.virtual_memory()
    return {
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
        "gate": gate.as_json(),
    }


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
