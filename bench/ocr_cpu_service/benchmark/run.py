"""Serial, bounded, model-neutral OCR benchmark runner."""

from __future__ import annotations

import argparse
import hashlib
import importlib.metadata
import json
import math
import os
import platform
import select
import signal
import subprocess
import sys
import time
from collections.abc import Mapping, Set
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Protocol

import psutil

from benchmark.candidates import CommandCandidateSpec
from benchmark.corpus import (
    DEFAULT_CORPUS,
    DEFAULT_MANIFEST,
    BenchmarkPage,
    generate_reviewed_multicolumn_case,
    inspect_and_render_historical,
    inspect_and_render_official,
    load_quantitative_pages,
)
from benchmark.metrics import error_counts, reading_order_violations
from benchmark.report import aggregate_records, recompute_and_validate_summary
from benchmark.worker import DEFAULT_MAX_OUTPUT_BYTES
from corpus.download import load_sources

ROOT = Path(__file__).resolve().parents[3]
SERVICE_ROOT = ROOT / "bench" / "ocr_cpu_service"
DEFAULT_WORK = SERVICE_ROOT / ".data" / "benchmark"
OFFICIAL_SOURCE_ID = "official-89-2026-tt-btc"
FILECONV_BUILD_COMMAND = (
    "CC=gcc CXX=g++ cargo build --release "
    "-p fileconv-cli --no-default-features"
)


def sanitized_candidate_environment(*, cpu_threads: int) -> dict[str, str]:
    """Return the complete allowlisted environment shared by candidates."""
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


class CandidateOutputLimitError(RuntimeError):
    """A candidate exceeded its hard stdout or stderr cap."""


class Candidate(Protocol):
    id: str
    label: str
    metadata: dict[str, Any]

    def recognize(
        self,
        page: BenchmarkPage,
        *,
        timeout_seconds: float | None = None,
    ) -> RecognitionMeasurement:
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


def _terminate_process_group(
    process: subprocess.Popen[str], *, grace_seconds: float = 0.5
) -> None:
    """Terminate a worker session, including descendants surviving SIGTERM."""
    process_group = process.pid

    def group_exists() -> bool:
        try:
            os.killpg(process_group, 0)
        except ProcessLookupError:
            return False
        return True

    try:
        os.killpg(process_group, signal.SIGTERM)
    except ProcessLookupError:
        pass

    deadline = time.monotonic() + grace_seconds
    while group_exists() and time.monotonic() < deadline:
        try:
            process.wait(timeout=min(0.05, max(0.0, deadline - time.monotonic())))
        except subprocess.TimeoutExpired:
            pass
        time.sleep(0.01)

    if group_exists():
        try:
            os.killpg(process_group, signal.SIGKILL)
        except ProcessLookupError:
            pass
    process.wait()


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
            _terminate_process_group(process)
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
    """One candidate in a dedicated, consistently monitored process session."""

    def __init__(
        self,
        *,
        candidate_id: str,
        label: str,
        command: list[str],
        environment: dict[str, str],
        timeout_seconds: float,
        max_output_bytes: int = DEFAULT_MAX_OUTPUT_BYTES,
        command_description: str = "isolated candidate worker",
        provenance: dict[str, Any] | None = None,
    ) -> None:
        if max_output_bytes <= 0:
            raise ValueError("max_output_bytes must be positive")
        self.id = candidate_id
        self.label = label
        self._timeout_seconds = timeout_seconds
        self._closed = False
        process_invoked = time.perf_counter()
        self._process = subprocess.Popen(
            command,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            bufsize=1,
            env=environment,
            start_new_session=True,
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
            "environment_variable_names": sorted(environment),
            "output_limits": {
                "stdout_bytes": max_output_bytes,
                "stderr_bytes": max_output_bytes,
                "enforcement": "bounded_pipes_with_process_tree_termination",
            },
            "cold_initialization": {
                "candidate_seconds": event["candidate_seconds"],
                "wall_seconds": cold_wall_seconds,
                "timing_scope": "worker_process_invocation_to_ready",
                "process_startup_included": True,
                "rss_measurement": resource,
            },
            **_json_compatible(provenance or {}),
        }

    def recognize(
        self,
        page: BenchmarkPage,
        *,
        timeout_seconds: float | None = None,
    ) -> RecognitionMeasurement:
        if self._process.stdin is None:
            raise RuntimeError("candidate worker stdin is unavailable")
        if timeout_seconds is not None and (
            isinstance(timeout_seconds, bool)
            or not isinstance(timeout_seconds, (int, float))
            or not math.isfinite(float(timeout_seconds))
            or timeout_seconds <= 0
        ):
            raise ValueError("timeout_seconds must be finite and positive")
        effective_timeout = min(
            self._timeout_seconds,
            (
                float(timeout_seconds)
                if timeout_seconds is not None
                else self._timeout_seconds
            ),
        )
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
            self._process, timeout_seconds=effective_timeout
        )
        if event.get("event") == "failure":
            if event.get("error_kind") == "output_limit":
                raise CandidateOutputLimitError
            raise RuntimeError("candidate worker reported a sanitized failure")
        if event.get("event") != "result":
            raise RuntimeError("candidate worker returned an invalid event")
        return RecognitionMeasurement(
            text=event["text"],
            candidate_seconds=event["candidate_seconds"],
            resource=resource,
        )

    def close(self) -> None:
        if self._closed:
            return
        try:
            if self._process.poll() is None:
                if self._process.stdin is not None:
                    try:
                        self._process.stdin.write('{"event":"shutdown"}\n')
                        self._process.stdin.flush()
                    except BrokenPipeError:
                        pass
                try:
                    self._process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    pass
            _terminate_process_group(self._process)
        finally:
            self._closed = True


def _isolated_worker(
    spec: CommandCandidateSpec,
    *,
    timeout_seconds: float,
    max_output_bytes: int = DEFAULT_MAX_OUTPUT_BYTES,
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
            "--max-output-bytes",
            str(max_output_bytes),
        ],
        environment=dict(spec.environment),
        timeout_seconds=timeout_seconds,
        max_output_bytes=max_output_bytes,
        command_description="python -m benchmark.worker --argv-json <candidate argv>",
        provenance=_json_compatible(spec.provenance),
    )


def run_candidate(
    spec: CommandCandidateSpec,
    page: BenchmarkPage,
    *,
    timeout_seconds: float,
    max_rss_bytes: int,
    max_output_bytes: int = DEFAULT_MAX_OUTPUT_BYTES,
) -> CandidateRunResult:
    """Run one arbitrary candidate without retaining recognized text."""
    candidate = _isolated_worker(
        spec,
        timeout_seconds=timeout_seconds,
        max_output_bytes=max_output_bytes,
    )
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
        except CandidateOutputLimitError:
            record["error_kind"] = "output_limit"
        except Exception:
            record["error_kind"] = "candidate_error"
        records.append(record)
        print(
            f"[{candidate.id}] {page.source_id} p{page.page_number}: "
            f"{'ok' if record['success'] else record['error_kind']}",
            file=sys.stderr,
            flush=True,
        )
    quantitative = [record for record in records if record["gate_included"]]
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


def _fileconv_candidate(
    *,
    candidate_id: str,
    label: str,
    fileconv: Path,
    tessdata: Path,
    tessdata_role: str,
    environment: dict[str, str],
    fileconv_build: dict[str, Any],
) -> CommandCandidateSpec:
    return CommandCandidateSpec(
        id=candidate_id,
        label=label,
        argv=(str(fileconv), "one", "{input}", "--lang", "vie+eng"),
        environment={
            **environment,
            "FILECONV_TESSDATA": str(tessdata),
        },
        provenance={
            "fileconv_build": fileconv_build,
            "invocation": "fileconv one <identical-page.png> --lang vie+eng",
            "timing_note": (
                "warm timing includes a fresh fileconv/Tesseract subprocess "
                "spawn, execution, and bounded output collection for every page"
            ),
            "tessdata_role": tessdata_role,
            "tessdata_sha256": {
                language: _sha256(tessdata / f"{language}.traineddata")
                for language in ("vie", "eng")
            },
        },
    )


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
    historical, historical_pages = inspect_and_render_historical(
        [source for source in sources if source.kind == "wikimedia-scan"],
        corpus_dir,
        args.work_dir.resolve(),
    )
    multicolumn, multicolumn_page = generate_reviewed_multicolumn_case(
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
    environment = sanitized_candidate_environment(cpu_threads=args.cpu_threads)
    fileconv_build = _fileconv_provenance(fileconv)
    candidates = [
        _fileconv_candidate(
            candidate_id="markhand-default",
            label="Markhand default",
            fileconv=fileconv,
            tessdata=system_tessdata,
            tessdata_role="system-default",
            environment=environment,
            fileconv_build=fileconv_build,
        ),
        _fileconv_candidate(
            candidate_id="markhand-tessdata-best",
            label="Markhand tessdata_best",
            fileconv=fileconv,
            tessdata=best_tessdata,
            tessdata_role="best",
            environment=environment,
            fileconv_build=fileconv_build,
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
                            "10 ms sampled sum of worker and descendant RSS; "
                            "max_rss_bytes is a measured gate, not an OS limit"
                        ),
                        "output": (
                            "stdout and stderr use hard per-stream bounded-pipe "
                            "collection with process-tree termination"
                        ),
                    },
                },
            ),
            timeout_seconds=args.timeout_seconds,
            max_output_bytes=args.max_output_bytes,
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
                "max_rss_enforcement": "measured_gate_only_not_os_enforced",
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
    parser.add_argument(
        "--max-output-bytes",
        type=int,
        default=DEFAULT_MAX_OUTPUT_BYTES,
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
