#!/usr/bin/env python3
"""Bounded bitonal PDF calibration runner for Thông tư 89 OCR spike."""

from __future__ import annotations

import argparse
import hashlib
import importlib.metadata
import json
import platform
import re
import shutil
import statistics
import subprocess
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import psutil
import pypdfium2 as pdfium

from benchmark.candidates import CommandCandidateSpec
from benchmark.corpus import BenchmarkPage
from benchmark.metrics import error_counts
from benchmark.render import PageRenderRejected, RenderLimits, render_page
from benchmark.run import (
    CandidateOutputLimitError,
    _isolated_worker,
    sanitized_candidate_environment,
)
from corpus.download import load_sources

ROOT = Path(__file__).resolve().parents[3]
SERVICE_ROOT = ROOT / "bench" / "ocr_cpu_service"
DEFAULT_CONFIG = SERVICE_ROOT / "experiments" / "bitonal-configs.json"
DEFAULT_SOURCES = SERVICE_ROOT / "corpus" / "sources.json"
DEFAULT_OUTPUT = SERVICE_ROOT / ".data" / "bitonal-pdf" / "calibration.json"
CALIBRATION_RUN_ROOT = SERVICE_ROOT / ".data" / "bitonal-pdf" / "runs"
OFFICIAL_SOURCE_ID = "official-89-2026-tt-btc"
FILECONV_BUILD_COMMAND = (
    "CC=gcc CXX=g++ cargo build --release "
    "-p fileconv-cli --no-default-features"
)
INVALID_PREPROCESS_MODE_MSG = "unsupported OCR preprocess mode"

EXPECTED_CANDIDATE_IDS = (
    "baseline-system-fast",
    "baseline-best",
    "bitonal-best-vie-eng",
    "bitonal-best-vie",
)
ALLOWED_ERROR_KINDS = frozenset({"timeout", "output_limit", "candidate_error"})
_COUNT_FIELDS = frozenset(
    {
        "character_edits",
        "reference_characters",
        "word_edits",
        "reference_words",
    }
)
_DIAGNOSTIC_FIELDS = frozenset(
    {
        "digit_sequence_count",
        "digit_sequence_checksum",
        "legal_identifier_count",
        "non_whitespace_character_count",
        "suspicious_character_count",
        "accent_proxy_counts",
    }
)
_BINDINGS_FIELDS = frozenset(
    {
        "source_sha256",
        "config_sha256",
        "binary_sha256",
        "pdfium_sha256",
        "toolchain_sha256",
        "tessdata_sha256",
        "render_sha256",
    }
)
_PROVENANCE_FIELDS = frozenset(
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
_ARTIFACT_FIELDS = frozenset(
    {
        "schema_version",
        "split",
        "page_count",
        "provenance",
        "host",
        "toolchain",
        "limits",
        "access",
        "render_hashes",
        "candidates",
        "records",
    }
)
_LIMITS_FIELDS = frozenset(
    {
        "cpu_threads",
        "timeout_seconds_per_page",
        "max_output_bytes_per_stream",
        "max_rss_bytes",
        "max_source_bytes",
        "process_tree_sample_interval_ms",
    }
)
_RENDER_FIELDS = frozenset({"dpi", "max_pixels", "max_dimension"})
_ACCESS_FIELDS = frozenset(
    {
        "approved_pages_opened",
        "holdout_pages_opened",
        "rendered_pages",
        "ocr_executions",
    }
)
_HOST_FIELDS = frozenset(
    {
        "platform",
        "architecture",
        "logical_cpus",
        "memory_bytes",
        "max_rss_bytes",
        "max_rss_enforcement",
    }
)
_TOOLCHAIN_FIELDS = frozenset({"cargo", "pypdfium2", "python", "tesseract"})
_CANDIDATE_FIELDS = frozenset(
    {
        "id",
        "mode",
        "tessdata",
        "langs",
        "argv",
        "environment_variable_names",
        "aggregate",
    }
)
_AGGREGATE_FIELDS = frozenset(
    {
        "pages",
        "successes",
        "failures",
        "latency_seconds",
        "peak_rss_bytes",
        "resource_limit_violations",
    }
)
_LATENCY_FIELDS = frozenset({"median", "total"})
_RECORD_BASE_FIELDS = frozenset(
    {
        "candidate_id",
        "page_number",
        "success",
        "bindings",
        "elapsed_seconds",
        "peak_rss_bytes",
        "resource_limit_violation",
    }
)
_CONFIG_FIELDS = frozenset(
    {
        "schema_version",
        "split",
        "expected_pages",
        "approved_page_numbers",
        "source",
        "render",
        "limits",
        "candidates",
        "page_diagnostics",
        "reference_disagreement",
    }
)
_SOURCE_FIELDS = frozenset({"id", "expected_sha256", "max_bytes"})
_REFERENCE_DISAGREEMENT_FIELDS = frozenset({"metric_label", "forbidden_labels"})
_FORBIDDEN_KEYS = frozenset(
    {
        "accuracy",
        "cer",
        "env",
        "environment",
        "environment_values",
        "ground_truth",
        "hypothesis",
        "recognised_text",
        "recognized_output",
        "recognized_text",
        "reference",
        "reference_text",
        "stderr",
        "stdout",
        "text",
        "transcription",
        "wer",
    }
)
_PLAN_LIMITS = {
    "cpu_threads": 1,
    "timeout_seconds_per_page": 180,
    "max_output_bytes_per_stream": 1_048_576,
    "max_rss_bytes": 805_306_368,
    "max_source_bytes": 17_281_751,
    "process_tree_sample_interval_ms": 10,
}
_PLAN_RENDER = {
    "dpi": 300,
    "max_pixels": 50_000_000,
    "max_dimension": 10_000,
}
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
    spec: CommandCandidateSpec


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


def _reject_forbidden_keys(value: Any, *, path: str = "artifact") -> None:
    if isinstance(value, dict):
        for key, item in value.items():
            if key.lower() in _FORBIDDEN_KEYS:
                raise ValueError(f"forbidden field at {path}.{key}")
            _reject_forbidden_keys(item, path=f"{path}.{key}")
    elif isinstance(value, list):
        for index, item in enumerate(value):
            _reject_forbidden_keys(item, path=f"{path}[{index}]")


def _validate_closed_mapping(
    value: Any,
    *,
    allowed: frozenset[str],
    path: str,
) -> None:
    if not isinstance(value, dict):
        raise ValueError(f"{path} must be a mapping")
    actual = set(value)
    if actual != allowed:
        unknown = sorted(actual - allowed)
        if unknown:
            raise ValueError(f"unknown field at {path}: {unknown[0]}")
        missing = sorted(allowed - actual)
        raise ValueError(f"missing field at {path}: {missing[0]}")


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
    _validate_closed_mapping(config, allowed=_CONFIG_FIELDS, path="config")
    if config["schema_version"] != 1 or config["split"] != "calibration":
        raise ValueError("calibration config must select schema 1 calibration only")
    if config["expected_pages"] != 22:
        raise ValueError("calibration config must lock 22 approved pages")
    approved_calibration_pages(config)
    _validate_closed_mapping(config["source"], allowed=_SOURCE_FIELDS, path="config.source")
    if config["source"]["id"] != OFFICIAL_SOURCE_ID:
        raise ValueError("calibration source identity is invalid")
    _validate_closed_mapping(config["render"], allowed=_RENDER_FIELDS, path="config.render")
    if config["render"] != _PLAN_RENDER:
        raise ValueError("render bounds exceed approved calibration limits")
    _validate_closed_mapping(config["limits"], allowed=_LIMITS_FIELDS, path="config.limits")
    if config["limits"] != _PLAN_LIMITS:
        raise ValueError("resource limits must match approved calibration constants")
    if config["source"]["max_bytes"] != config["limits"]["max_source_bytes"]:
        raise ValueError("source byte cap does not match limits")
    _validate_closed_mapping(
        config["reference_disagreement"],
        allowed=_REFERENCE_DISAGREEMENT_FIELDS,
        path="config.reference_disagreement",
    )
    candidate_ids = tuple(candidate["id"] for candidate in config["candidates"])
    if candidate_ids != EXPECTED_CANDIDATE_IDS:
        raise ValueError("calibration candidate IDs are invalid")
    for candidate in config["candidates"]:
        _validate_candidate_config(candidate)
    _reject_forbidden_keys(config)
    return config


def _validate_candidate_config(candidate: dict[str, Any]) -> None:
    allowed = frozenset(
        {
            "id",
            "mode",
            "tessdata",
            "langs",
            "argv_template",
            "environment_variable_names",
        }
    )
    _validate_closed_mapping(candidate, allowed=allowed, path="config.candidate")
    if candidate["mode"] not in {"legacy", "preserve-near-bitonal"}:
        raise ValueError("candidate preprocess mode is invalid")
    if candidate["tessdata"] not in {"system", "best"}:
        raise ValueError("candidate tessdata role is invalid")
    template = candidate["argv_template"]
    if (
        template[:2] != ["{fileconv}", "one"]
        or template[2] != "{input}"
        or template[3] != "--lang"
        or template[4] != candidate["langs"]
    ):
        raise ValueError("candidate argv template is invalid")
    names = tuple(candidate["environment_variable_names"])
    if list(names) != sorted(names):
        raise ValueError("candidate environment variable names must be sorted")
    if candidate["mode"] == "preserve-near-bitonal":
        if "FILECONV_OCR_PREPROCESS_MODE" not in names:
            raise ValueError("bitonal candidate must declare preprocess mode env name")
    elif "FILECONV_OCR_PREPROCESS_MODE" in names:
        raise ValueError("legacy candidate must not declare preprocess mode env name")


def strip_for_disagreement(text: str) -> str:
    """Remove page comments, Markdown decoration, and standalone page numbers."""
    without_comments = _PAGE_COMMENT_RE.sub(" ", text)
    without_markdown = _MARKDOWN_DECORATION_RE.sub(" ", without_comments)
    without_page_numbers = _STANDALONE_PAGE_NUMBER_RE.sub(" ", without_markdown)
    return " ".join(without_page_numbers.split())


strip_reference_for_disagreement = strip_for_disagreement


def load_private_reference_pages(path: Path, pages: tuple[int, ...]) -> dict[int, str]:
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
        page: "\n".join(sections.get(page, []))
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
    return sum(len(re.findall(pattern, text, flags=re.MULTILINE)) for pattern in patterns)


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
    cleaned_reference = strip_for_disagreement(reference)
    cleaned_hypothesis = strip_for_disagreement(hypothesis)
    counts = error_counts(cleaned_reference, cleaned_hypothesis)
    return {
        "character_edits": counts.character_edits,
        "reference_characters": counts.reference_characters,
        "word_edits": counts.word_edits,
        "reference_words": counts.reference_words,
    }


def allocate_calibration_work_dir() -> Path:
    CALIBRATION_RUN_ROOT.mkdir(parents=True, exist_ok=True)
    return Path(tempfile.mkdtemp(prefix="run-", dir=CALIBRATION_RUN_ROOT))


def release_calibration_work_dir(path: Path) -> None:
    resolved = path.resolve()
    root = CALIBRATION_RUN_ROOT.resolve()
    if resolved == root:
        raise ValueError("refusing to delete calibration run root")
    try:
        resolved.relative_to(root)
    except ValueError as error:
        raise ValueError("refusing to delete path outside calibration run root") from error
    if resolved.exists():
        shutil.rmtree(resolved)


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
        expected_names = tuple(candidate["environment_variable_names"])
        actual_names = tuple(sorted(environment))
        if actual_names != expected_names:
            raise ValueError(
                f"candidate {candidate['id']} environment variable names drifted"
            )
        argv = tuple(
            str(fileconv) if token == "{fileconv}" else token
            for token in candidate["argv_template"]
        )
        spec = CommandCandidateSpec(
            id=candidate["id"],
            label=candidate["id"],
            argv=argv,
            environment=environment,
            provenance={"mode": candidate["mode"], "tessdata": candidate["tessdata"]},
        )
        candidates.append(
            CalibrationCandidate(
                id=candidate["id"],
                mode=candidate["mode"],
                tessdata=candidate["tessdata"],
                langs=candidate["langs"],
                argv=argv,
                environment_variable_names=expected_names,
                spec=spec,
            )
        )
    return candidates


def _verify_official_pdf(
    pdf_path: Path, *, expected_sha256: str, max_bytes: int
) -> None:
    size = pdf_path.stat().st_size
    if size > max_bytes:
        raise ValueError("official PDF exceeds approved byte cap")
    actual = _sha256(pdf_path)
    if actual != expected_sha256:
        raise ValueError("official PDF checksum mismatch")


def render_calibration_pages(
    pdf_path: Path,
    work_dir: Path,
    *,
    config: dict[str, Any],
) -> dict[int, tuple[Path, str]]:
    render = config["render"]
    limits = RenderLimits(
        dpi=render["dpi"],
        max_pixels=render["max_pixels"],
        max_dimension=render["max_dimension"],
    )
    pages = approved_calibration_pages(config)
    work_dir.mkdir(parents=True, exist_ok=True)
    rendered: dict[int, tuple[Path, str]] = {}
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
            rendered[page_number] = (output, _sha256(output))
    finally:
        document.close()
    return rendered


def recognize_calibration_page(
    candidate: CalibrationCandidate,
    *,
    page_path: Path,
    page_number: int,
    timeout_seconds: float,
    max_output_bytes: int,
    max_rss_bytes: int,
) -> tuple[str | None, dict[str, Any], str | None]:
    """Run one page through the reviewed bounded worker contract."""
    worker = _isolated_worker(
        candidate.spec,
        timeout_seconds=timeout_seconds,
        max_output_bytes=max_output_bytes,
    )
    try:
        measurement = worker.recognize(
            BenchmarkPage(
                source_id=OFFICIAL_SOURCE_ID,
                source_sha256="",
                stratum="calibration",
                page_number=page_number,
                path=page_path,
                reference=None,
            )
        )
        peak_rss = int(measurement.resource["peak_rss_bytes"])
        resource = {
            "elapsed_seconds": float(measurement.resource["wall_seconds"]),
            "peak_rss_bytes": peak_rss,
            "resource_limit_violation": peak_rss > max_rss_bytes,
        }
        return measurement.text, resource, None
    except TimeoutError:
        return None, {"elapsed_seconds": 0.0, "peak_rss_bytes": 0, "resource_limit_violation": False}, "timeout"
    except CandidateOutputLimitError:
        return None, {"elapsed_seconds": 0.0, "peak_rss_bytes": 0, "resource_limit_violation": False}, "output_limit"
    except Exception:
        return None, {"elapsed_seconds": 0.0, "peak_rss_bytes": 0, "resource_limit_violation": False}, "candidate_error"
    finally:
        worker.close()


def _record_bindings(
    *,
    provenance: dict[str, Any],
    tessdata_role: str,
    render_sha256: str,
) -> dict[str, Any]:
    return {
        "source_sha256": provenance["source_sha256"],
        "config_sha256": provenance["config_sha256"],
        "binary_sha256": provenance["binary_sha256"],
        "pdfium_sha256": provenance["pdfium_sha256"],
        "toolchain_sha256": provenance["toolchain_sha256"],
        "tessdata_sha256": provenance["tessdata_sha256"][tessdata_role],
        "render_sha256": render_sha256,
    }


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
    if successes and all(
        _COUNT_FIELDS <= set(record) for record in successes if record["page_number"] <= 20
    ):
        reference_successes = [
            record for record in successes if record["page_number"] <= 20
        ]
        if reference_successes:
            aggregate["raw_counts"] = {
                field: sum(int(record[field]) for record in reference_successes)
                for field in _COUNT_FIELDS
            }
    return aggregate


def _validate_bindings(
    bindings: Any, *, provenance: dict[str, Any], tessdata_role: str, render_sha256: str
) -> None:
    _validate_closed_mapping(bindings, allowed=_BINDINGS_FIELDS, path="record.bindings")
    if bindings["source_sha256"] != provenance["source_sha256"]:
        raise ValueError("record source binding is inconsistent")
    if bindings["config_sha256"] != provenance["config_sha256"]:
        raise ValueError("record config binding is inconsistent")
    if bindings["binary_sha256"] != provenance["binary_sha256"]:
        raise ValueError("record binary binding is inconsistent")
    if bindings["pdfium_sha256"] != provenance["pdfium_sha256"]:
        raise ValueError("record pdfium binding is inconsistent")
    if bindings["toolchain_sha256"] != provenance["toolchain_sha256"]:
        raise ValueError("record toolchain binding is inconsistent")
    if bindings["tessdata_sha256"] != provenance["tessdata_sha256"][tessdata_role]:
        raise ValueError("record tessdata binding is inconsistent")
    if bindings["render_sha256"] != render_sha256:
        raise ValueError("record render binding is inconsistent")


def validate_calibration_artifact(payload: dict[str, Any]) -> None:
    _validate_closed_mapping(payload, allowed=_ARTIFACT_FIELDS, path="artifact")
    if payload["schema_version"] != 1:
        raise ValueError("calibration artifact schema is invalid")
    if payload["split"] != "calibration":
        raise ValueError("calibration artifact split is invalid")
    if payload["page_count"] != 22:
        raise ValueError("calibration artifact must contain exactly 22 pages")
    _validate_closed_mapping(
        payload["provenance"], allowed=_PROVENANCE_FIELDS, path="artifact.provenance"
    )
    provenance = payload["provenance"]
    if not all(_valid_checksum(value) for value in provenance.values()):
        raise ValueError("calibration provenance contains an invalid checksum")
    if set(provenance["tessdata_sha256"]) != {"system", "best"}:
        raise ValueError("tessdata provenance roles are invalid")
    _validate_closed_mapping(payload["host"], allowed=_HOST_FIELDS, path="artifact.host")
    _validate_closed_mapping(
        payload["toolchain"], allowed=_TOOLCHAIN_FIELDS, path="artifact.toolchain"
    )
    if provenance["host_sha256"] != _canonical_sha256(payload["host"]):
        raise ValueError("host checksum does not bind the host descriptor")
    if provenance["toolchain_sha256"] != _canonical_sha256(payload["toolchain"]):
        raise ValueError("toolchain checksum does not bind all versions")
    _validate_closed_mapping(payload["limits"], allowed=_LIMITS_FIELDS, path="artifact.limits")
    if payload["limits"] != _PLAN_LIMITS:
        raise ValueError("calibration limits must match approved constants")
    _validate_closed_mapping(payload["access"], allowed=_ACCESS_FIELDS, path="artifact.access")
    if payload["access"] != {
        "approved_pages_opened": 22,
        "holdout_pages_opened": 0,
        "rendered_pages": 22,
        "ocr_executions": 88,
    }:
        raise ValueError("calibration access evidence is invalid")
    render_hashes = payload["render_hashes"]
    if not isinstance(render_hashes, dict):
        raise ValueError("render hashes are invalid")
    expected_render_pages = {str(page) for page in range(1, 21)} | {"60", "450"}
    if set(render_hashes) != expected_render_pages:
        raise ValueError("render hashes must cover approved pages only")
    if not all(_valid_checksum(value) for value in render_hashes.values()):
        raise ValueError("render hashes contain an invalid checksum")
    _reject_forbidden_keys(payload)
    candidates = payload["candidates"]
    if tuple(candidate["id"] for candidate in candidates) != EXPECTED_CANDIDATE_IDS:
        raise ValueError("calibration candidate IDs are invalid")
    config = load_calibration_config()
    config_by_id = {candidate["id"]: candidate for candidate in config["candidates"]}
    for candidate in candidates:
        _validate_closed_mapping(
            candidate, allowed=_CANDIDATE_FIELDS, path="artifact.candidate"
        )
        expected = config_by_id[candidate["id"]]
        if (
            candidate["mode"] != expected["mode"]
            or candidate["tessdata"] != expected["tessdata"]
            or candidate["langs"] != expected["langs"]
        ):
            raise ValueError("candidate semantics drifted from frozen config")
        if candidate["argv"][1:] != [
            "one",
            "{input}",
            "--lang",
            expected["langs"],
        ]:
            raise ValueError("candidate argv template drifted from frozen config")
        if candidate["environment_variable_names"] != expected["environment_variable_names"]:
            raise ValueError("candidate environment variable names drifted")
        aggregate_fields = frozenset(candidate["aggregate"])
        allowed_aggregate = (
            _AGGREGATE_FIELDS | frozenset({"raw_counts"})
            if "raw_counts" in aggregate_fields
            else _AGGREGATE_FIELDS
        )
        _validate_closed_mapping(
            candidate["aggregate"],
            allowed=allowed_aggregate,
            path="artifact.candidate.aggregate",
        )
        _validate_closed_mapping(
            candidate["aggregate"]["latency_seconds"],
            allowed=_LATENCY_FIELDS,
            path="artifact.candidate.aggregate.latency_seconds",
        )
    records = payload["records"]
    if not isinstance(records, list):
        raise ValueError("calibration records cardinality is invalid")
    approved_pages = set(range(1, 21)) | {60, 450}
    seen: set[tuple[str, int]] = set()
    for record in records:
        if not isinstance(record, dict):
            raise ValueError("calibration record must be a mapping")
        _reject_forbidden_keys(record, path="record")
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
        tessdata_role = config_by_id[candidate_id]["tessdata"]
        render_sha256 = render_hashes[str(page_number)]
        _validate_bindings(
            record["bindings"],
            provenance=provenance,
            tessdata_role=tessdata_role,
            render_sha256=render_sha256,
        )
        if record["success"]:
            allowed = _RECORD_BASE_FIELDS | (
                _COUNT_FIELDS if page_number <= 20 else frozenset({"diagnostics"})
            )
            _validate_closed_mapping(record, allowed=allowed, path="record")
            if page_number <= 20:
                _validate_closed_mapping(
                    {key: record[key] for key in _COUNT_FIELDS},
                    allowed=_COUNT_FIELDS,
                    path="record.counts",
                )
            else:
                _validate_closed_mapping(
                    record["diagnostics"],
                    allowed=_DIAGNOSTIC_FIELDS,
                    path="record.diagnostics",
                )
        else:
            allowed = _RECORD_BASE_FIELDS | frozenset({"error_kind"})
            _validate_closed_mapping(record, allowed=allowed, path="record")
            if record["error_kind"] not in ALLOWED_ERROR_KINDS:
                raise ValueError("failure record error_kind is invalid")
    if len(records) != 88:
        raise ValueError("calibration records cardinality is invalid")
    expected_keys = {
        (candidate_id, page_number)
        for candidate_id in EXPECTED_CANDIDATE_IDS
        for page_number in approved_pages
    }
    if seen != expected_keys:
        raise ValueError("candidate-page records are missing")
    per_page_renders = {
        page: {
            record["bindings"]["render_sha256"]
            for record in records
            if record["page_number"] == page
        }
        for page in approved_pages
    }
    if any(len(values) != 1 for values in per_page_renders.values()):
        raise ValueError("all candidates for a page must share one render hash")


def run_calibration(args: argparse.Namespace) -> dict[str, Any]:
    config = load_calibration_config(args.config.resolve())
    sources = load_sources(args.sources.resolve())
    official = next(source for source in sources if source.id == OFFICIAL_SOURCE_ID)
    if official.sha256 != config["source"]["expected_sha256"]:
        raise ValueError("manifest source checksum does not match frozen config")
    pdf_path = args.pdf.resolve()
    _verify_official_pdf(
        pdf_path,
        expected_sha256=config["source"]["expected_sha256"],
        max_bytes=config["limits"]["max_source_bytes"],
    )
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
    )
    work_dir = allocate_calibration_work_dir()
    records: list[dict[str, Any]] = []
    render_hashes: dict[str, str] = {}
    try:
        rendered_pages = render_calibration_pages(pdf_path, work_dir, config=config)
        render_hashes = {
            str(page_number): page_hash
            for page_number, (_, page_hash) in rendered_pages.items()
        }
        for candidate in candidates:
            for page_number, (page_path, render_sha256) in sorted(
                rendered_pages.items()
            ):
                bindings = _record_bindings(
                    provenance=provenance,
                    tessdata_role=candidate.tessdata,
                    render_sha256=render_sha256,
                )
                text, resource, error_kind = recognize_calibration_page(
                    candidate,
                    page_path=page_path,
                    page_number=page_number,
                    timeout_seconds=config["limits"]["timeout_seconds_per_page"],
                    max_output_bytes=config["limits"]["max_output_bytes_per_stream"],
                    max_rss_bytes=config["limits"]["max_rss_bytes"],
                )
                record: dict[str, Any] = {
                    "candidate_id": candidate.id,
                    "page_number": page_number,
                    "success": error_kind is None,
                    "bindings": bindings,
                    "elapsed_seconds": resource["elapsed_seconds"],
                    "peak_rss_bytes": resource["peak_rss_bytes"],
                    "resource_limit_violation": resource["resource_limit_violation"],
                }
                if error_kind is None:
                    assert text is not None
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
                    del text
                else:
                    record["error_kind"] = error_kind
                records.append(record)
    finally:
        release_calibration_work_dir(work_dir)
    public_candidates = [
        {
            "id": candidate.id,
            "mode": candidate.mode,
            "tessdata": candidate.tessdata,
            "langs": candidate.langs,
            "argv": ["{fileconv}", "one", "{input}", "--lang", candidate.langs],
            "environment_variable_names": list(candidate.environment_variable_names),
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
        "render_hashes": render_hashes,
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
