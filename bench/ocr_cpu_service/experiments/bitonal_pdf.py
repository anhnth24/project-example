#!/usr/bin/env python3
"""Bounded bitonal PDF calibration runner for Thông tư 89 OCR spike."""

from __future__ import annotations

import argparse
import hashlib
import importlib.metadata
import json
import platform
import re
import select
import shutil
import statistics
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import psutil
import pypdfium2 as pdfium

from benchmark.candidates import CommandCandidateSpec, render_argv
from benchmark.metrics import error_counts
from benchmark.render import PageRenderRejected, RenderLimits, render_page
from benchmark.run import (
    CandidateOutputLimitError,
    _terminate_process_group,
    sanitized_candidate_environment,
)
from corpus.download import load_sources

ROOT = Path(__file__).resolve().parents[3]
SERVICE_ROOT = ROOT / "bench" / "ocr_cpu_service"
DEFAULT_CONFIG = SERVICE_ROOT / "experiments" / "bitonal-configs.json"
DEFAULT_SOURCES = SERVICE_ROOT / "corpus" / "sources.json"
DEFAULT_CORPUS = SERVICE_ROOT / ".data" / "corpus"
DEFAULT_OUTPUT = SERVICE_ROOT / ".data" / "bitonal-pdf" / "calibration.json"
OFFICIAL_SOURCE_ID = "official-89-2026-tt-btc"
FILECONV_BUILD_COMMAND = (
    "CC=gcc CXX=g++ cargo build --release "
    "-p fileconv-cli --no-default-features"
)

EXPECTED_CANDIDATE_IDS = (
    "baseline-system-fast",
    "baseline-best",
    "bitonal-best-vie-eng",
    "bitonal-best-vie",
)
_COUNT_FIELDS = (
    "character_edits",
    "reference_characters",
    "word_edits",
    "reference_words",
)
_DIAGNOSTIC_FIELDS = (
    "digit_sequence_count",
    "digit_sequence_checksum",
    "legal_identifier_count",
    "non_whitespace_character_count",
    "suspicious_character_count",
    "accent_proxy_counts",
)
_CHECKSUM_FIELDS = frozenset(
    {
        "source_sha256",
        "config_sha256",
        "binary_sha256",
        "tessdata_sha256",
        "pdfium_sha256",
        "host_sha256",
        "toolchain_sha256",
    }
)
_FORBIDDEN_TEXT_KEYS = frozenset(
    {
        "recognized_text",
        "recognised_text",
        "hypothesis",
        "transcription",
        "reference",
        "reference_text",
        "text",
        "stdout",
        "stderr",
    }
)
_ALLOWED_LIMIT_KEYS = frozenset(
    {
        "cpu_threads",
        "timeout_seconds_per_page",
        "max_output_bytes_per_stream",
        "max_rss_bytes",
        "process_tree_sample_interval_ms",
    }
)
_ALLOWED_RENDER_KEYS = frozenset({"dpi", "max_pixels", "max_dimension"})
_SUSPICIOUS_CHARACTER_RE = re.compile(
    r"[^\w\s\u00C0-\u024F\u1E00-\u1EFF.,;:!?()\-+/%\"'“”‘’]"
)
_PAGE_COMMENT_RE = re.compile(r"<!--.*?-->", re.DOTALL)
_MARKDOWN_DECORATION_RE = re.compile(r"[*_`#>\[\]()|]")
_STANDALONE_PAGE_NUMBER_RE = re.compile(r"^\s*\d{1,4}\s*$", re.MULTILINE)


@dataclass(frozen=True, slots=True)
class CalibrationCandidate:
    id: str
    mode: str
    tessdata: str
    langs: str
    argv: tuple[str, ...]
    environment_variable_names: tuple[str, ...]
    provenance: dict[str, Any]


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _canonical_sha256(value: Any) -> str:
    payload = json.dumps(
        value, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode()
    return hashlib.sha256(payload).hexdigest()


def _valid_checksum(value: Any) -> bool:
    if isinstance(value, str):
        return len(value) == 64 and all(
            character in "0123456789abcdef" for character in value
        )
    if isinstance(value, dict) and value:
        return all(
            isinstance(key, str) and _valid_checksum(item)
            for key, item in value.items()
        )
    return False


def _assert_no_recognized_text(value: Any, path: str = "artifact") -> None:
    if isinstance(value, dict):
        for key, item in value.items():
            if key.lower() in _FORBIDDEN_TEXT_KEYS:
                raise ValueError(f"recognized text field is forbidden at {path}.{key}")
            _assert_no_recognized_text(item, f"{path}.{key}")
    elif isinstance(value, list):
        for index, item in enumerate(value):
            _assert_no_recognized_text(item, f"{path}[{index}]")


def _assert_no_environment_values(value: Any, path: str = "artifact") -> None:
    if isinstance(value, dict):
        for key, item in value.items():
            if key in {"environment", "environment_values"}:
                raise ValueError(f"environment values are forbidden at {path}.{key}")
            _assert_no_environment_values(item, f"{path}.{key}")
    elif isinstance(value, list):
        for index, item in enumerate(value):
            _assert_no_environment_values(item, f"{path}[{index}]")


def approved_calibration_pages(config: dict[str, Any]) -> tuple[int, ...]:
    pages = tuple(config["approved_page_numbers"])
    if pages != tuple(range(1, 21)) + (60, 450):
        raise ValueError("approved calibration pages are invalid")
    return pages


def load_calibration_config(path: Path = DEFAULT_CONFIG) -> dict[str, Any]:
    try:
        config = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"{path}: invalid calibration config: {error}") from error
    required = {
        "schema_version",
        "split",
        "expected_pages",
        "approved_page_numbers",
        "render",
        "limits",
        "candidates",
        "page_diagnostics",
        "reference_disagreement",
    }
    if not isinstance(config, dict) or set(config) != required:
        raise ValueError("calibration config fields are invalid")
    if config["schema_version"] != 1 or config["split"] != "calibration":
        raise ValueError("calibration config must select schema 1 calibration only")
    if config["expected_pages"] != 22:
        raise ValueError("calibration config must lock 22 approved pages")
    approved_calibration_pages(config)
    if set(config["render"]) != _ALLOWED_RENDER_KEYS:
        raise ValueError("render bounds are not fully specified")
    render = config["render"]
    if (
        render["dpi"] != 300
        or render["max_pixels"] != 50_000_000
        or render["max_dimension"] != 10_000
    ):
        raise ValueError("render bounds exceed approved calibration limits")
    if set(config["limits"]) != _ALLOWED_LIMIT_KEYS:
        raise ValueError("resource limits are not fully bounded")
    limits = config["limits"]
    if (
        limits["cpu_threads"] <= 0
        or limits["timeout_seconds_per_page"] <= 0
        or limits["max_output_bytes_per_stream"] <= 0
        or limits["max_rss_bytes"] <= 0
        or limits["process_tree_sample_interval_ms"] != 10
    ):
        raise ValueError("resource limits must remain positive and bounded")
    candidate_ids = tuple(candidate["id"] for candidate in config["candidates"])
    if candidate_ids != EXPECTED_CANDIDATE_IDS:
        raise ValueError("calibration candidate IDs are invalid")
    for candidate in config["candidates"]:
        if candidate["mode"] not in {"legacy", "preserve-near-bitonal"}:
            raise ValueError("candidate preprocess mode is invalid")
        if candidate["tessdata"] not in {"system", "best"}:
            raise ValueError("candidate tessdata role is invalid")
        if "environment" in candidate:
            raise ValueError("calibration config must not serialize environment values")
    return config


def strip_reference_for_disagreement(text: str) -> str:
    """Remove page comments, Markdown decoration, and standalone page numbers."""
    without_comments = _PAGE_COMMENT_RE.sub(" ", text)
    without_markdown = _MARKDOWN_DECORATION_RE.sub(" ", without_comments)
    without_page_numbers = _STANDALONE_PAGE_NUMBER_RE.sub(" ", without_markdown)
    return " ".join(without_page_numbers.split())


def load_private_reference_pages(path: Path, pages: tuple[int, ...]) -> dict[int, str]:
    """Load per-page private reference text without retaining the full document."""
    if not path.is_file():
        raise ValueError("private reference path is required at runtime")
    sections: dict[int, list[str]] = {}
    current_page: int | None = None
    for line in path.read_text(encoding="utf-8").splitlines():
        marker = re.match(r"^<!--\s*page\s+(\d+)\s*-->\s*$", line.strip(), re.I)
        if marker:
            current_page = int(marker.group(1))
            sections.setdefault(current_page, [])
            continue
        if current_page is not None:
            sections[current_page].append(line)
    loaded = {
        page: strip_reference_for_disagreement("\n".join(sections.get(page, [])))
        for page in pages
        if page <= 20
    }
    if set(loaded) != set(range(1, 21)):
        raise ValueError("private reference must contain pages 1 through 20")
    return loaded


def _digit_sequence_metrics(text: str) -> tuple[int, str]:
    sequences = re.findall(r"\d+", text)
    checksum = hashlib.sha256(
        json.dumps(sequences, ensure_ascii=False, separators=(",", ":")).encode()
    ).hexdigest()
    return len(sequences), checksum


def _legal_identifier_count(text: str, patterns: list[str]) -> int:
    total = 0
    for pattern in patterns:
        total += len(re.findall(pattern, text, flags=re.MULTILINE))
    return total


def _non_whitespace_character_count(text: str) -> int:
    return sum(not character.isspace() for character in text)


def _suspicious_character_count(text: str) -> int:
    return len(_SUSPICIOUS_CHARACTER_RE.findall(text))


def _accent_proxy_counts(
    text: str, proxies: list[dict[str, Any]], *, note_text: str
) -> dict[str, int]:
    lowered = text.casefold()
    note_lowered = note_text.casefold()
    counts: dict[str, int] = {}
    for proxy in proxies:
        proxy_id = proxy["id"]
        tokens = proxy["tokens"]
        if not any(token.casefold() in note_lowered for token in tokens):
            raise ValueError(
                f"accent proxy {proxy_id} is missing from the supplied note"
            )
        counts[proxy_id] = sum(lowered.count(token.casefold()) for token in tokens)
    return counts


def page_diagnostics(
    text: str,
    *,
    page_number: int,
    config: dict[str, Any],
    note_text: str,
) -> dict[str, Any]:
    digit_count, digit_checksum = _digit_sequence_metrics(text)
    diagnostics: dict[str, Any] = {
        "digit_sequence_count": digit_count,
        "digit_sequence_checksum": digit_checksum,
        "legal_identifier_count": 0,
        "non_whitespace_character_count": _non_whitespace_character_count(text),
        "suspicious_character_count": _suspicious_character_count(text),
        "accent_proxy_counts": {},
    }
    page_config = config["page_diagnostics"].get(str(page_number), {})
    if page_number == 60:
        diagnostics["accent_proxy_counts"] = _accent_proxy_counts(
            text,
            page_config.get("accent_proxies", []),
            note_text=note_text,
        )
    if page_number == 450:
        diagnostics["legal_identifier_count"] = _legal_identifier_count(
            text,
            page_config.get("legal_identifier_patterns", []),
        )
    return diagnostics


def reference_disagreement_counts(reference: str, hypothesis: str) -> dict[str, int]:
    counts = error_counts(reference, hypothesis)
    return {
        "character_edits": counts.character_edits,
        "reference_characters": counts.reference_characters,
        "word_edits": counts.word_edits,
        "reference_words": counts.reference_words,
    }


def _tessdata_checksums(path: Path) -> dict[str, str]:
    return {
        language: _sha256(path / f"{language}.traineddata")
        for language in ("vie", "eng")
    }


def _command_version(command: list[str]) -> str:
    result = subprocess.run(
        command, capture_output=True, text=True, check=True, timeout=30
    )
    return (result.stdout or result.stderr).splitlines()[0].strip()


def _host_description(max_rss_bytes: int) -> dict[str, Any]:
    memory = psutil.virtual_memory()
    return {
        "platform": platform.platform(),
        "architecture": platform.machine(),
        "logical_cpus": psutil.cpu_count(logical=True),
        "memory_bytes": memory.total,
        "max_rss_bytes": max_rss_bytes,
        "max_rss_enforcement": "measured_gate_only_not_os_enforced",
    }


def _toolchain_description() -> dict[str, Any]:
    return {
        "cargo": _command_version(["cargo", "--version"]),
        "pypdfium2": importlib.metadata.version("pypdfium2"),
        "python": platform.python_version(),
        "tesseract": _command_version(["tesseract", "--version"]),
    }


def build_calibration_provenance(
    *,
    source_sha256: str,
    config_path: Path,
    fileconv: Path,
    system_tessdata: Path,
    best_tessdata: Path,
    pdfium_lib: Path,
    host: dict[str, Any],
    toolchain: dict[str, Any],
) -> dict[str, Any]:
    return {
        "source_sha256": source_sha256,
        "config_sha256": _sha256(config_path),
        "binary_sha256": _sha256(fileconv),
        "tessdata_sha256": {
            "system": _tessdata_checksums(system_tessdata),
            "best": _tessdata_checksums(best_tessdata),
        },
        "pdfium_sha256": _sha256(pdfium_lib / "libpdfium.so"),
        "host_sha256": _canonical_sha256(host),
        "toolchain_sha256": _canonical_sha256(toolchain),
    }


def _candidate_environment(
    *,
    candidate: dict[str, Any],
    base_environment: dict[str, str],
    system_tessdata: Path,
    best_tessdata: Path,
    pdfium_lib: Path,
) -> dict[str, str]:
    environment = dict(base_environment)
    environment["FILECONV_PDFIUM_LIB"] = str(pdfium_lib)
    tessdata = system_tessdata if candidate["tessdata"] == "system" else best_tessdata
    environment["FILECONV_TESSDATA"] = str(tessdata)
    if candidate["mode"] == "preserve-near-bitonal":
        environment["FILECONV_OCR_PREPROCESS_MODE"] = "preserve-near-bitonal"
    return environment


def build_calibration_candidates(
    config: dict[str, Any],
    *,
    fileconv: Path,
    system_tessdata: Path,
    best_tessdata: Path,
    pdfium_lib: Path,
    binary_sha256: str,
    tessdata_sha256: dict[str, dict[str, str]],
    toolchain_sha256: str,
) -> list[CalibrationCandidate]:
    base_environment = sanitized_candidate_environment(
        cpu_threads=config["limits"]["cpu_threads"]
    )
    candidates: list[CalibrationCandidate] = []
    for candidate in config["candidates"]:
        environment = _candidate_environment(
            candidate=candidate,
            base_environment=base_environment,
            system_tessdata=system_tessdata,
            best_tessdata=best_tessdata,
            pdfium_lib=pdfium_lib,
        )
        argv = (
            str(fileconv),
            "one",
            "{input}",
            "--lang",
            candidate["langs"],
        )
        candidates.append(
            CalibrationCandidate(
                id=candidate["id"],
                mode=candidate["mode"],
                tessdata=candidate["tessdata"],
                langs=candidate["langs"],
                argv=argv,
                environment_variable_names=tuple(sorted(environment)),
                provenance={
                    "binary_sha256": binary_sha256,
                    "toolchain_sha256": toolchain_sha256,
                    "mode": candidate["mode"],
                    "tessdata": {
                        "role": candidate["tessdata"],
                        "sha256": tessdata_sha256[
                            "system" if candidate["tessdata"] == "system" else "best"
                        ],
                    },
                    "invocation": (
                        f"fileconv one <rendered-page.png> --lang {candidate['langs']}"
                    ),
                },
            )
        )
    return candidates


def _verify_pdf_checksum(pdf_path: Path, expected_sha256: str) -> None:
    actual = _sha256(pdf_path)
    if actual != expected_sha256:
        raise ValueError("official PDF checksum mismatch")


def render_calibration_pages(
    pdf_path: Path,
    work_dir: Path,
    *,
    config: dict[str, Any],
) -> dict[int, Path]:
    render = config["render"]
    limits = RenderLimits(
        dpi=render["dpi"],
        max_pixels=render["max_pixels"],
        max_dimension=render["max_dimension"],
    )
    pages = approved_calibration_pages(config)
    work_dir.mkdir(parents=True, exist_ok=True)
    rendered: dict[int, Path] = {}
    document = pdfium.PdfDocument(pdf_path)
    try:
        for page_number in pages:
            output = work_dir / f"page-{page_number:04d}.png"
            source_page = document[page_number - 1]
            try:
                image = render_page(source_page, limits)
                try:
                    image.save(output, format="PNG")
                finally:
                    image.close()
            except PageRenderRejected as error:
                raise ValueError(
                    f"rendered page {page_number} exceeds approved bounds"
                ) from error
            finally:
                source_page.close()
            rendered[page_number] = output
    finally:
        document.close()
    return rendered


def _recognize_page_direct(
    candidate: CalibrationCandidate,
    *,
    input_path: Path,
    environment: dict[str, str],
    timeout_seconds: float,
    max_output_bytes: int,
    sample_interval_seconds: float,
) -> tuple[str, dict[str, Any]]:
    command = render_argv(
        CommandCandidateSpec(
            id=candidate.id,
            label=candidate.id,
            argv=candidate.argv,
            environment=environment,
            provenance=candidate.provenance,
        ),
        input_path,
    )
    process = subprocess.Popen(
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=environment,
        start_new_session=True,
    )
    try:
        if process.stdout is None or process.stderr is None:
            raise RuntimeError("candidate output pipes are unavailable")
        started = time.perf_counter()
        monitored = psutil.Process(process.pid)
        peak_rss = 0
        sample_count = 0
        stdout_chunks: list[str] = []
        stderr_chunks: list[str] = []
        while True:
            peak_rss = max(
                peak_rss,
                _process_tree_rss(monitored),
            )
            sample_count += 1
            if time.perf_counter() - started > timeout_seconds:
                _terminate_process_group(process)
                raise TimeoutError("candidate page timeout")
            readable, _, _ = select.select(
                [process.stdout, process.stderr], [], [], sample_interval_seconds
            )
            for stream in readable:
                chunk = stream.read(65536)
                if not chunk:
                    continue
                if stream is process.stdout:
                    stdout_chunks.append(chunk)
                    if sum(len(item) for item in stdout_chunks) > max_output_bytes:
                        _terminate_process_group(process)
                        raise CandidateOutputLimitError
                else:
                    stderr_chunks.append(chunk)
                    if sum(len(item) for item in stderr_chunks) > max_output_bytes:
                        _terminate_process_group(process)
                        raise CandidateOutputLimitError
            if process.poll() is not None and not readable:
                break
        if process.returncode != 0:
            raise RuntimeError("candidate command failed")
        text = "".join(stdout_chunks)
        resource = {
            "method": "sampled_process_tree_rss",
            "peak_rss_bytes": peak_rss,
            "sample_count": sample_count,
            "sample_interval_seconds": sample_interval_seconds,
            "wall_seconds": time.perf_counter() - started,
        }
        return text, resource
    finally:
        if process.poll() is None:
            _terminate_process_group(process)


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


def aggregate_calibration_records(records: list[dict[str, Any]]) -> dict[str, Any]:
    successes = [record for record in records if record["success"]]
    failures = [record for record in records if not record["success"]]
    elapsed = [float(record["elapsed_seconds"]) for record in successes]
    rss = [int(record["peak_rss_bytes"]) for record in successes]
    aggregate: dict[str, Any] = {
        "pages": len(records),
        "successes": len(successes),
        "failures": len(failures),
        "latency_seconds": {
            "median": statistics.median(elapsed) if elapsed else 0.0,
            "total": sum(elapsed),
        },
        "peak_rss_bytes": max(rss, default=0),
        "resource_limit_violations": sum(
            bool(record["resource_limit_violation"]) for record in records
        ),
    }
    if all(field in record for record in successes for field in _COUNT_FIELDS):
        aggregate["raw_counts"] = {
            field: sum(int(record[field]) for record in successes)
            for field in _COUNT_FIELDS
        }
    return aggregate


def validate_calibration_artifact(payload: dict[str, Any]) -> None:
    expected_fields = {
        "schema_version",
        "split",
        "page_count",
        "provenance",
        "host",
        "toolchain",
        "limits",
        "access",
        "candidates",
        "records",
    }
    if set(payload) != expected_fields or payload.get("schema_version") != 1:
        raise ValueError("calibration artifact schema is invalid")
    if payload.get("split") != "calibration":
        raise ValueError("calibration artifact split is invalid")
    if payload.get("page_count") != 22:
        raise ValueError("calibration artifact must contain exactly 22 pages")
    provenance = payload.get("provenance")
    if not isinstance(provenance, dict) or set(provenance) != _CHECKSUM_FIELDS:
        raise ValueError("calibration provenance checksum set is incomplete")
    if not all(_valid_checksum(value) for value in provenance.values()):
        raise ValueError("calibration provenance contains an invalid checksum")
    if set(provenance["tessdata_sha256"]) != {"system", "best"}:
        raise ValueError("tessdata provenance roles are invalid")
    if provenance["host_sha256"] != _canonical_sha256(payload["host"]):
        raise ValueError("host checksum does not bind the host descriptor")
    if provenance["toolchain_sha256"] != _canonical_sha256(payload["toolchain"]):
        raise ValueError("toolchain checksum does not bind all versions")
    _assert_no_recognized_text(payload)
    _assert_no_environment_values(payload)
    limits = payload.get("limits")
    if not isinstance(limits, dict) or set(limits) != _ALLOWED_LIMIT_KEYS:
        raise ValueError("calibration limits are not fully bounded")
    if any(
        not isinstance(limits[key], int) or limits[key] <= 0
        for key in (
            "cpu_threads",
            "timeout_seconds_per_page",
            "max_output_bytes_per_stream",
            "max_rss_bytes",
        )
    ):
        raise ValueError("calibration limits must remain positive and bounded")
    if limits["process_tree_sample_interval_ms"] != 10:
        raise ValueError("process-tree sampling interval must remain bounded")
    access = payload.get("access")
    if access != {
        "approved_pages_opened": 22,
        "holdout_pages_opened": 0,
        "rendered_pages": 22,
        "ocr_executions": 88,
    }:
        raise ValueError("calibration access evidence is invalid")
    candidates = payload.get("candidates")
    if (
        not isinstance(candidates, list)
        or tuple(candidate.get("id") for candidate in candidates)
        != EXPECTED_CANDIDATE_IDS
    ):
        raise ValueError("calibration candidate IDs are invalid")
    records = payload.get("records")
    if not isinstance(records, list):
        raise ValueError("calibration records cardinality is invalid")
    approved_pages = set(range(1, 21)) | {60, 450}
    seen: set[tuple[str, int]] = set()
    for record in records:
        candidate_id = record.get("candidate_id")
        page_number = record.get("page_number")
        if candidate_id not in EXPECTED_CANDIDATE_IDS:
            raise ValueError("calibration record candidate is invalid")
        if page_number not in approved_pages:
            raise ValueError("approved calibration pages only")
        key = (candidate_id, page_number)
        if key in seen:
            raise ValueError("duplicate candidate-page record")
        seen.add(key)
    if len(records) != 88:
        raise ValueError("calibration records cardinality is invalid")
    for record in records:
        candidate_id = record.get("candidate_id")
        page_number = record.get("page_number")
        if not _valid_checksum(record.get("render_sha256")):
            raise ValueError("render checksum is invalid")
        for candidate in candidates:
            if candidate["id"] != candidate_id:
                continue
            if record.get("mode") != candidate.get("mode"):
                raise ValueError("record mode does not match candidate")
            if record.get("langs") != candidate.get("langs"):
                raise ValueError("record langs do not match candidate")
        if page_number <= 20:
            if any(field not in record for field in _COUNT_FIELDS):
                raise ValueError("reference disagreement counts are incomplete")
            if "diagnostics" in record:
                raise ValueError("reference pages must not store diagnostics")
        else:
            diagnostics = record.get("diagnostics")
            if not isinstance(diagnostics, dict) or set(diagnostics) != set(
                _DIAGNOSTIC_FIELDS
            ):
                raise ValueError("diagnostic page fields are incomplete")
            if any(field in record for field in _COUNT_FIELDS):
                raise ValueError("diagnostic pages must not store reference counts")
    expected_keys = {
        (candidate_id, page_number)
        for candidate_id in EXPECTED_CANDIDATE_IDS
        for page_number in approved_pages
    }
    if seen != expected_keys:
        raise ValueError("candidate-page records are missing")


def run_calibration(args: argparse.Namespace) -> dict[str, Any]:
    config = load_calibration_config(args.config.resolve())
    sources = load_sources(args.sources.resolve())
    official = next(source for source in sources if source.id == OFFICIAL_SOURCE_ID)
    pdf_path = args.pdf.resolve()
    _verify_pdf_checksum(pdf_path, official.sha256)
    reference_pages = load_private_reference_pages(
        args.reference.resolve(),
        approved_calibration_pages(config),
    )
    note_text = args.note.read_text(encoding="utf-8")
    fileconv = args.fileconv.resolve()
    system_tessdata = args.system_tessdata.resolve()
    best_tessdata = args.best_tessdata.resolve()
    pdfium_lib = args.pdfium_lib.resolve()
    host = _host_description(config["limits"]["max_rss_bytes"])
    toolchain = _toolchain_description()
    provenance = build_calibration_provenance(
        source_sha256=official.sha256,
        config_path=args.config.resolve(),
        fileconv=fileconv,
        system_tessdata=system_tessdata,
        best_tessdata=best_tessdata,
        pdfium_lib=pdfium_lib,
        host=host,
        toolchain=toolchain,
    )
    candidates = build_calibration_candidates(
        config,
        fileconv=fileconv,
        system_tessdata=system_tessdata,
        best_tessdata=best_tessdata,
        pdfium_lib=pdfium_lib,
        binary_sha256=provenance["binary_sha256"],
        tessdata_sha256=provenance["tessdata_sha256"],
        toolchain_sha256=provenance["toolchain_sha256"],
    )
    work_dir = args.work_dir.resolve()
    rendered_pages = render_calibration_pages(pdf_path, work_dir, config=config)
    records: list[dict[str, Any]] = []
    try:
        for candidate in candidates:
            environment = _candidate_environment(
                candidate={
                    "mode": candidate.mode,
                    "tessdata": candidate.tessdata,
                    "langs": candidate.langs,
                },
                base_environment=sanitized_candidate_environment(
                    cpu_threads=config["limits"]["cpu_threads"]
                ),
                system_tessdata=system_tessdata,
                best_tessdata=best_tessdata,
                pdfium_lib=pdfium_lib,
            )
            for page_number, page_path in sorted(rendered_pages.items()):
                record: dict[str, Any] = {
                    "candidate_id": candidate.id,
                    "page_number": page_number,
                    "mode": candidate.mode,
                    "langs": candidate.langs,
                    "render_sha256": _sha256(page_path),
                    "success": False,
                    "error_kind": None,
                    "elapsed_seconds": 0.0,
                    "peak_rss_bytes": 0,
                    "resource_limit_violation": False,
                }
                try:
                    text, resource = _recognize_page_direct(
                        candidate,
                        input_path=page_path,
                        environment=environment,
                        timeout_seconds=config["limits"]["timeout_seconds_per_page"],
                        max_output_bytes=config["limits"]["max_output_bytes_per_stream"],
                        sample_interval_seconds=(
                            config["limits"]["process_tree_sample_interval_ms"] / 1000
                        ),
                    )
                    if page_number <= 20:
                        record.update(
                            reference_disagreement_counts(
                                reference_pages[page_number], text
                            )
                        )
                    else:
                        record["diagnostics"] = page_diagnostics(
                            text,
                            page_number=page_number,
                            config=config,
                            note_text=note_text,
                        )
                    peak_rss = int(resource["peak_rss_bytes"])
                    record.update(
                        success=True,
                        elapsed_seconds=float(resource["wall_seconds"]),
                        peak_rss_bytes=peak_rss,
                        resource_limit_violation=peak_rss
                        > config["limits"]["max_rss_bytes"],
                    )
                    del text
                except TimeoutError:
                    record["error_kind"] = "timeout"
                except CandidateOutputLimitError:
                    record["error_kind"] = "output_limit"
                except Exception:
                    record["error_kind"] = "candidate_error"
                records.append(record)
    finally:
        shutil.rmtree(work_dir, ignore_errors=True)
    public_candidates = [
        {
            "id": candidate.id,
            "mode": candidate.mode,
            "tessdata": candidate.tessdata,
            "langs": candidate.langs,
            "argv": list(candidate.argv),
            "environment_variable_names": list(candidate.environment_variable_names),
            "provenance": candidate.provenance,
            "aggregate": aggregate_calibration_records(
                [record for record in records if record["candidate_id"] == candidate.id]
            ),
        }
        for candidate in candidates
    ]
    artifact = {
        "schema_version": 1,
        "split": "calibration",
        "page_count": 22,
        "provenance": provenance,
        "host": host,
        "toolchain": toolchain,
        "limits": config["limits"],
        "access": {
            "approved_pages_opened": 22,
            "holdout_pages_opened": 0,
            "rendered_pages": 22,
            "ocr_executions": 88,
        },
        "candidates": public_candidates,
        "records": records,
    }
    validate_calibration_artifact(artifact)
    return artifact


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    calibrate = subparsers.add_parser("calibrate")
    calibrate.add_argument("--pdf", type=Path, required=True)
    calibrate.add_argument("--reference", type=Path, required=True)
    calibrate.add_argument("--note", type=Path, required=True)
    calibrate.add_argument("--fileconv", type=Path, required=True)
    calibrate.add_argument("--pdfium-lib", type=Path, required=True)
    calibrate.add_argument("--system-tessdata", type=Path, required=True)
    calibrate.add_argument("--best-tessdata", type=Path, required=True)
    calibrate.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
    calibrate.add_argument("--sources", type=Path, default=DEFAULT_SOURCES)
    calibrate.add_argument("--work-dir", type=Path, default=DEFAULT_OUTPUT.parent / "work")
    calibrate.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    return parser


def main() -> None:
    args = _parser().parse_args()
    if args.command == "calibrate":
        artifact = run_calibration(args)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(
            json.dumps(artifact, ensure_ascii=False, indent=2) + "\n",
            encoding="utf-8",
        )


if __name__ == "__main__":
    main()
