"""Deterministic, tuning-only OCR baseline experiment runner."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import platform
import statistics
import subprocess
import tomllib
from dataclasses import asdict
from pathlib import Path
from typing import Any, Callable

import psutil

from benchmark.candidates import CommandCandidateSpec
from benchmark.corpus import BenchmarkPage
from benchmark.metrics import error_counts
from benchmark.run import (
    CandidateOutputLimitError,
    CandidateResourceLimitError,
    CandidateResourceSamplingError,
    _isolated_worker,
    sanitized_candidate_environment,
)
from corpus.split import load_accuracy_annotations

ROOT = Path(__file__).resolve().parents[3]
SERVICE_ROOT = ROOT / "bench" / "ocr_cpu_service"
DEFAULT_CONFIG = SERVICE_ROOT / "experiments" / "baseline.json"
DEFAULT_ANNOTATIONS = SERVICE_ROOT / "corpus" / "accuracy-annotations.jsonl"
DEFAULT_SOURCES = SERVICE_ROOT / "corpus" / "accuracy-sources.json"
DEFAULT_ASSETS = SERVICE_ROOT / ".data" / "corpus"
EXPECTED_CANDIDATE_IDS = (
    "worker-system-fast",
    "markhand-auto",
    "tessdata-best",
)
_CHECKSUM_FIELDS = frozenset(
    {
        "source_sha256",
        "split_sha256",
        "config_sha256",
        "binary_sha256",
        "tessdata_sha256",
        "host_sha256",
        "toolchain_sha256",
    }
)
_COUNT_FIELDS = (
    "character_edits",
    "reference_characters",
    "word_edits",
    "reference_words",
)
_FORBIDDEN_TEXT_KEYS = frozenset(
    {
        "recognized_text",
        "recognised_text",
        "hypothesis",
        "transcription",
        "reference",
        "text",
        "stdout",
        "stderr",
    }
)


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


def load_baseline_config(path: Path = DEFAULT_CONFIG) -> dict[str, Any]:
    try:
        config = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"{path}: invalid baseline config: {error}") from error
    required = {
        "schema_version",
        "split",
        "expected_pages",
        "repetitions",
        "limits",
        "candidates",
        "determinism",
    }
    if not isinstance(config, dict) or set(config) != required:
        raise ValueError("baseline config fields are invalid")
    if config["schema_version"] != 1 or config["split"] != "tuning":
        raise ValueError("baseline config must select schema 1 tuning only")
    if config["expected_pages"] != 44 or config["repetitions"] != 2:
        raise ValueError("baseline config must lock 44 tuning pages and two repetitions")
    if tuple(candidate.get("id") for candidate in config["candidates"]) != (
        EXPECTED_CANDIDATE_IDS
    ):
        raise ValueError("baseline config candidate IDs are invalid")
    if tuple(
        candidate.get("tessdata_mode") for candidate in config["candidates"]
    ) != ("system-fast", "auto", "best"):
        raise ValueError("baseline config tessdata semantics are invalid")
    if any("environment" in candidate for candidate in config["candidates"]):
        raise ValueError("baseline config must not serialize environment values")
    return config


def _tessdata_checksums(path: Path) -> dict[str, str]:
    return {
        language: _sha256(path / f"{language}.traineddata")
        for language in ("vie", "eng")
    }


def build_run_provenance(
    *,
    source_manifest: Path,
    split_payload: bytes,
    config_path: Path,
    fileconv: Path,
    tessdata_paths: dict[str, Path],
    host: dict[str, Any],
    toolchain: dict[str, Any],
) -> dict[str, Any]:
    return {
        "source_sha256": _sha256(source_manifest),
        "split_sha256": hashlib.sha256(split_payload).hexdigest(),
        "config_sha256": _sha256(config_path),
        "binary_sha256": _sha256(fileconv),
        "tessdata_sha256": {
            role: _tessdata_checksums(path)
            for role, path in sorted(tessdata_paths.items())
        },
        "host_sha256": _canonical_sha256(host),
        "toolchain_sha256": _canonical_sha256(toolchain),
    }


def resolve_auto_tessdata(
    *,
    cwd: Path,
    executable: Path,
    manifest_dir: Path,
) -> Path:
    """Mirror fileconv-core's no-override tessdata_best search order."""
    roots = [
        *list(cwd.resolve().parents)[:3],
    ]
    roots.insert(0, cwd.resolve())
    executable_parent = executable.resolve().parent
    roots.extend([executable_parent, *list(executable_parent.parents)[:3]])
    manifest = manifest_dir.resolve()
    roots.extend([manifest, *list(manifest.parents)[:3]])
    for root in roots:
        candidate = root / "tessdata_best"
        if (candidate / "vie.traineddata").is_file():
            return candidate
    raise ValueError("Markhand auto-discovery did not resolve tessdata_best")


def build_candidate_specs(
    config: dict[str, Any],
    *,
    fileconv: Path,
    system_tessdata: Path,
    best_tessdata: Path,
    auto_tessdata: Path,
    cpu_threads: int,
    binary_sha256: str,
    tessdata_sha256: dict[str, dict[str, str]],
    toolchain_sha256: str,
) -> tuple[list[CommandCandidateSpec], list[dict[str, Any]]]:
    base_environment = sanitized_candidate_environment(cpu_threads=cpu_threads)
    substitutions = {
        "{fileconv}": str(fileconv),
    }
    role_paths = {
        "system-fast": system_tessdata,
        "auto": auto_tessdata,
        "best": best_tessdata,
    }
    specs: list[CommandCandidateSpec] = []
    public: list[dict[str, Any]] = []
    for candidate in config["candidates"]:
        argv = tuple(substitutions.get(value, value) for value in candidate["argv"])
        role = candidate["tessdata_mode"]
        candidate_environment = (
            {}
            if role == "auto"
            else {"FILECONV_TESSDATA": str(role_paths[role])}
        )
        environment = {**base_environment, **candidate_environment}
        tessdata = {
            "mode": role,
            "resolved_path": str(auto_tessdata) if role == "auto" else None,
            "sha256": tessdata_sha256[role],
        }
        provenance = {
            "binary_sha256": binary_sha256,
            "toolchain_sha256": toolchain_sha256,
            "tessdata": tessdata,
        }
        specs.append(
            CommandCandidateSpec(
                id=candidate["id"],
                label=candidate["label"],
                argv=argv,
                environment=environment,
                provenance=provenance,
            )
        )
        public.append(
            {
                "id": candidate["id"],
                "label": candidate["label"],
                "argv": list(argv),
                "environment_variable_names": sorted(environment),
                "provenance": provenance,
            }
        )
    return specs, public


def select_tuning_pages(
    rows: list[Any],
    *,
    assets_dir: Path,
    expected_pages: int,
    checksum: Callable[[Path], str] = _sha256,
) -> tuple[list[dict[str, Any]], bytes]:
    normalized = [asdict(row) if not isinstance(row, dict) else dict(row) for row in rows]
    selected = sorted(
        (row for row in normalized if row["split"] == "tuning"),
        key=lambda row: row["page_id"],
    )
    if len(selected) != expected_pages:
        raise ValueError(
            f"tuning split has {len(selected)} pages, expected {expected_pages}"
        )
    split_binding: list[dict[str, Any]] = []
    pages: list[dict[str, Any]] = []
    for row in selected:
        asset = assets_dir / row["asset_path"]
        actual = checksum(asset)
        if actual != row["image_sha256"]:
            raise ValueError(f"tuning asset checksum mismatch: {row['page_id']}")
        pages.append({**row, "path": asset})
        split_binding.append(
            {
                "page_id": row["page_id"],
                "source_id": row["source_id"],
                "source_sha256": row["source_sha256"],
                "page_number": row["page_number"],
                "asset_path": row["asset_path"],
                "image_sha256": row["image_sha256"],
                "transcription_sha256": hashlib.sha256(
                    row["transcription"].encode()
                ).hexdigest(),
                "difficulty_strata": sorted(row["difficulty_strata"]),
            }
        )
    payload = json.dumps(
        split_binding, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode()
    return pages, payload


def aggregate_records(records: list[dict[str, Any]]) -> dict[str, Any]:
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
            "p95": _percentile(elapsed, 0.95),
            "total": sum(elapsed),
        },
        "peak_rss_bytes": max(rss, default=0),
        "resource_limit_violations": sum(
            bool(record["resource_limit_violation"]) for record in records
        ),
    }


def _percentile(values: list[float], quantile: float) -> float:
    if not values:
        return 0.0
    index = max(0, math.ceil(quantile * len(values)) - 1)
    return values[index]


def _run_one_candidate(
    spec: CommandCandidateSpec,
    public: dict[str, Any],
    pages: list[dict[str, Any]],
    *,
    timeout_seconds: float,
    max_output_bytes: int,
    max_rss_bytes: int,
    diagnostic_limit: int,
) -> dict[str, Any]:
    worker = _isolated_worker(
        spec,
        timeout_seconds=timeout_seconds,
        max_rss_bytes=max_rss_bytes,
        max_output_bytes=max_output_bytes,
    )
    records: list[dict[str, Any]] = []
    try:
        for page in pages:
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
                )
            except TimeoutError:
                record["error_kind"] = "timeout"
            except CandidateOutputLimitError:
                record["error_kind"] = "output_limit"
            except (CandidateResourceLimitError, CandidateResourceSamplingError):
                record["error_kind"] = "resource_limit"
                record["resource_limit_violation"] = True
            except Exception:
                record["error_kind"] = "candidate_error"
            records.append(record)
    finally:
        worker.close()
    strata = {
        stratum: aggregate_records(
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
    diagnostics = sorted(
        records,
        key=lambda record: (
            -(
                record.get("character_edits", 0)
                / max(1, record.get("reference_characters", 0))
            ),
            record["page_id"],
        ),
    )[:diagnostic_limit]
    return {
        **public,
        "measurement": {
            "cold_initialization": worker.metadata["cold_initialization"],
            "timing": (
                "per-page wall time from warm worker request through result; "
                "fresh fileconv subprocess execution is included"
            ),
            "rss": (
                "10 ms sampled process-tree RSS; max_rss_bytes is a measured "
                "gate, not an OS-enforced limit"
            ),
            "output": (
                "stdout and stderr each use hard bounded-pipe collection with "
                "process-tree termination"
            ),
            "timeout": "per-page wall deadline; timed-out process groups are terminated",
        },
        "aggregate": aggregate_records(records),
        "strata": strata,
        "records": records,
        "diagnostics": diagnostics,
    }


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


def _valid_checksum_value(value: Any) -> bool:
    if isinstance(value, str):
        return len(value) == 64 and all(
            character in "0123456789abcdef" for character in value
        )
    if isinstance(value, dict) and value:
        return all(
            isinstance(key, str) and _valid_checksum_value(item)
            for key, item in value.items()
        )
    return False


def validate_run_artifact(artifact: dict[str, Any]) -> None:
    expected_fields = {
        "schema_version",
        "repetition",
        "split",
        "page_count",
        "provenance",
        "host",
        "toolchain",
        "access",
        "candidates",
    }
    if set(artifact) != expected_fields or artifact.get("schema_version") != 1:
        raise ValueError("run artifact schema is invalid")
    if artifact.get("repetition") not in {1, 2}:
        raise ValueError("run artifact repetition must be 1 or 2")
    if artifact.get("split") != "tuning":
        raise ValueError("run artifact must contain tuning only")
    if artifact.get("page_count") != 44:
        raise ValueError("run artifact must contain exactly 44 tuning pages")
    provenance = artifact.get("provenance")
    if not isinstance(provenance, dict) or set(provenance) != _CHECKSUM_FIELDS:
        raise ValueError("run provenance checksum set is incomplete")
    if not all(_valid_checksum_value(value) for value in provenance.values()):
        raise ValueError("run provenance contains an invalid checksum")
    if set(provenance["tessdata_sha256"]) != {"system-fast", "auto", "best"}:
        raise ValueError("tessdata provenance roles are invalid")
    if provenance["host_sha256"] != _canonical_sha256(artifact["host"]):
        raise ValueError("host checksum does not bind the host descriptor")
    if provenance["toolchain_sha256"] != _canonical_sha256(artifact["toolchain"]):
        raise ValueError("toolchain checksum does not bind all versions")
    _assert_no_recognized_text(artifact)
    _assert_no_environment_values(artifact)
    access = artifact.get("access")
    if access != {
        "selected_tuning_pages": 44,
        "tuning_assets_checksummed": 44,
        "holdout_assets_resolved": 0,
        "holdout_assets_checksummed": 0,
        "holdout_assets_opened": 0,
        "holdout_ocr_executions": 0,
    }:
        raise ValueError("holdout non-access evidence or tuning access count is invalid")
    candidates = artifact.get("candidates")
    if (
        not isinstance(candidates, list)
        or tuple(candidate.get("id") for candidate in candidates)
        != EXPECTED_CANDIDATE_IDS
    ):
        raise ValueError("run artifact candidate IDs are invalid")
    expected_page_ids: set[str] | None = None
    role_by_id = dict(
        zip(EXPECTED_CANDIDATE_IDS, ("system-fast", "auto", "best"), strict=True)
    )
    for candidate in candidates:
        records = candidate["records"]
        page_ids = [record.get("page_id") for record in records]
        if len(records) != 44:
            raise ValueError("candidate must contain 44 unique tuning records")
        if len(set(page_ids)) != 44:
            raise ValueError("candidate records contain a duplicate page")
        if expected_page_ids is None:
            expected_page_ids = set(page_ids)
        elif set(page_ids) != expected_page_ids:
            raise ValueError("candidate records have missing or unexpected pages")
        if any(record.get("split") != "tuning" for record in records):
            raise ValueError("holdout record evidence is forbidden")
        successes = sum(record.get("success") is True for record in records)
        failures = sum(record.get("success") is False for record in records)
        aggregate = candidate["aggregate"]
        if (
            successes + failures != 44
            or aggregate.get("pages") != 44
            or aggregate.get("successes") != successes
            or aggregate.get("failures") != failures
        ):
            raise ValueError("candidate success/failure cardinality is inconsistent")
        for record in records:
            if record["success"]:
                if record.get("error_kind") is not None or any(
                    field not in record for field in _COUNT_FIELDS
                ):
                    raise ValueError("success record fields are inconsistent")
            elif (
                not isinstance(record.get("error_kind"), str)
                or not record["error_kind"]
                or any(field in record for field in _COUNT_FIELDS)
            ):
                raise ValueError("failure record fields are inconsistent")
        if len(candidate["diagnostics"]) > 8:
            raise ValueError("bounded page diagnostics limit exceeded")
        diagnostic_ids = [record.get("page_id") for record in candidate["diagnostics"]]
        if len(diagnostic_ids) != len(set(diagnostic_ids)) or not set(
            diagnostic_ids
        ) <= set(page_ids):
            raise ValueError("bounded diagnostics are not a unique page subset")
        if candidate["aggregate"] != aggregate_records(records):
            raise ValueError("candidate aggregate does not match raw records")
        candidate_provenance = candidate["provenance"]
        role = role_by_id[candidate["id"]]
        if (
            candidate_provenance.get("binary_sha256")
            != provenance["binary_sha256"]
            or candidate_provenance.get("toolchain_sha256")
            != provenance["toolchain_sha256"]
            or candidate_provenance.get("tessdata", {}).get("mode") != role
            or candidate_provenance.get("tessdata", {}).get("sha256")
            != provenance["tessdata_sha256"][role]
        ):
            raise ValueError("candidate provenance is inconsistent")
        resolved_path = candidate_provenance["tessdata"].get("resolved_path")
        if (role == "auto") != (
            isinstance(resolved_path, str) and bool(resolved_path)
        ):
            raise ValueError("auto tessdata resolution provenance is invalid")
        expected_strata = {
            stratum
            for record in records
            for stratum in record["difficulty_strata"]
        }
        if set(candidate["strata"]) != expected_strata:
            raise ValueError("candidate stratum aggregates are incomplete")
        for stratum, aggregate in candidate["strata"].items():
            expected = aggregate_records(
                [
                    record
                    for record in records
                    if stratum in record["difficulty_strata"]
                ]
            )
            if aggregate != expected:
                raise ValueError(f"stratum aggregate does not match raw records: {stratum}")


def _deterministic_aggregate(aggregate: dict[str, Any]) -> dict[str, Any]:
    return {
        key: value
        for key, value in aggregate.items()
        if key not in {"latency_seconds", "peak_rss_bytes", "resource_limit_violations"}
    }


def _deterministic_record(record: dict[str, Any]) -> dict[str, Any]:
    return {
        key: value
        for key, value in record.items()
        if key
        not in {
            "elapsed_seconds",
            "candidate_seconds",
            "peak_rss_bytes",
            "rss_sample_count",
            "resource_limit_violation",
        }
    }


def _deterministic_projection(artifact: dict[str, Any]) -> dict[str, Any]:
    return {
        "schema_version": artifact["schema_version"],
        "repetition": artifact["repetition"],
        "split": artifact["split"],
        "page_count": artifact["page_count"],
        "provenance": artifact["provenance"],
        "host": artifact["host"],
        "toolchain": artifact["toolchain"],
        "access": artifact["access"],
        "candidates": [
            {
                "id": candidate["id"],
                "label": candidate["label"],
                "argv": candidate["argv"],
                "environment_variable_names": candidate[
                    "environment_variable_names"
                ],
                "provenance": candidate["provenance"],
                "records": [
                    _deterministic_record(record)
                    for record in candidate["records"]
                ],
                "aggregate": _deterministic_aggregate(candidate["aggregate"]),
                "strata": {
                    stratum: _deterministic_aggregate(aggregate)
                    for stratum, aggregate in candidate["strata"].items()
                },
                "diagnostics": [
                    _deterministic_record(record)
                    for record in candidate["diagnostics"]
                ],
                "measurement": {
                    key: value
                    for key, value in candidate["measurement"].items()
                    if key != "cold_initialization"
                },
            }
            for candidate in artifact["candidates"]
        ],
    }


def compare_repetitions(
    first: dict[str, Any], second: dict[str, Any]
) -> dict[str, Any]:
    validate_run_artifact(first)
    validate_run_artifact(second)
    if (first["repetition"], second["repetition"]) != (1, 2):
        raise ValueError("comparison requires repetitions 1 and 2 in order")
    if first["provenance"] != second["provenance"]:
        raise ValueError("candidate or run provenance changed between repetitions")
    left = _deterministic_projection(first)
    right = _deterministic_projection(second)
    left["repetition"] = None
    right["repetition"] = None
    if left != right:
        raise ValueError("nondeterministic OCR count or candidate interface detected")
    timing_variance: dict[str, Any] = {}
    for left_candidate, right_candidate in zip(
        first["candidates"], second["candidates"], strict=True
    ):
        times = [
            float(record["elapsed_seconds"])
            for candidate in (left_candidate, right_candidate)
            for record in candidate["records"]
            if record["success"]
        ]
        timing_variance[left_candidate["id"]] = {
            "min_seconds": min(times, default=0.0),
            "max_seconds": max(times, default=0.0),
            "median_seconds": statistics.median(times) if times else 0.0,
        }
    return {
        "deterministic_counts": True,
        "timing_compared_for_determinism": False,
        "timing_variance": timing_variance,
    }


def render_baseline_report(
    first: dict[str, Any], second: dict[str, Any]
) -> str:
    comparison = compare_repetitions(first, second)
    lines = [
        "# Vietnamese OCR tuning baseline",
        "",
        "## Decision and scope",
        "",
        "The baseline is accepted for tuning experiments: OCR edit counts and "
        "provenance were identical across both repetitions. Timing is measured "
        "but explicitly excluded from determinism.",
        "",
        "**Holdout assets were not read or executed.** No holdout image was "
        "checksummed, opened, or passed to OCR in these runs. The six historical "
        "holdout pages remain untouched until the one-time Task 5 gate.",
        "",
        "This 44-page tuning sample is provisional; **production remains blocked** "
        "because the holdout is not representative of modern documents.",
        "",
        "Tracked artifacts contain no recognized output text. Per-page rows below "
        "contain additive edit counts only.",
        "",
        "## Immutable provenance",
        "",
        "| Check | SHA-256 |",
        "| --- | --- |",
    ]
    for name, value in first["provenance"].items():
        rendered = (
            json.dumps(value, sort_keys=True, separators=(",", ":"))
            if isinstance(value, dict)
            else value
        )
        lines.append(f"| `{name}` | `{rendered}` |")
    if "host" in first:
        lines.extend(
            [
                "",
                "Host descriptor bound by `host_sha256`:",
                "",
                f"- Platform: `{first['host']['platform']}`",
                f"- Architecture: `{first['host']['architecture']}`",
                f"- Logical CPUs: {first['host']['logical_cpus']}",
                f"- Physical CPUs: {first['host']['physical_cpus']}",
                f"- Memory bytes: {first['host']['memory_bytes']}",
                f"- Tesseract: `{first['toolchain']['tesseract'][0]}`",
                f"- Python: `{first['toolchain']['python']['implementation']} "
                f"{first['toolchain']['python']['version']}`",
                f"- fileconv package: "
                f"`{first['toolchain']['fileconv']['version']}`",
                f"- Cargo: `{first['toolchain']['cargo']}`",
                f"- Rust: `{first['toolchain']['rustc'][0]}`",
                f"- C compiler: `{first['toolchain']['cc']['tool']}` "
                f"(`{first['toolchain']['cc']['version']}`)",
                f"- C++ compiler: `{first['toolchain']['cxx']['tool']}` "
                f"(`{first['toolchain']['cxx']['version']}`)",
                "- Build environment variable names: `CC`, `CXX`; profile: "
                "`release`; features: `no-default-features`.",
            ]
        )
    lines.extend(
        [
            "",
            "## Candidate interfaces",
            "",
            "| Candidate | Semantics | Exact argv | Environment variable names |",
            "| --- | --- | --- | --- |",
        ]
    )
    semantics = {
        "worker-system-fast": (
            "Explicit system tessdata matching the current worker Docker "
            "deployment without bundled tessdata_best"
        ),
        "markhand-auto": (
            "No FILECONV_TESSDATA override; core checkout auto-discovery"
        ),
        "tessdata-best": "Explicit repository tessdata_best override",
    }
    for candidate in first["candidates"]:
        argv = json.dumps(candidate["argv"], ensure_ascii=False)
        names = ", ".join(
            f"`{name}`" for name in candidate["environment_variable_names"]
        )
        lines.append(
            f"| `{candidate['id']}` | {semantics[candidate['id']]} | "
            f"`{argv}` | {names} |"
        )
    auto = next(
        candidate
        for candidate in first["candidates"]
        if candidate["id"] == "markhand-auto"
    )
    lines.extend(
        [
            "",
            "Only variable names are recorded; environment values are omitted.",
            "The auto-discovered tessdata path was "
            f"`{auto['provenance']['tessdata']['resolved_path']}` and its "
            "language-file checksums are bound in candidate provenance.",
            "",
            "Deployment and checkout behavior differ intentionally: the current "
            "worker image has no bundled tessdata_best and explicitly selects "
            "system-fast, while this repository checkout auto-discovers its local "
            "tessdata_best when no override is present.",
            "",
            "## Holdout non-access evidence",
            "",
            "| Selected tuning | Tuning checksums | Holdout resolved | "
            "Holdout checksums | Holdout opened | Holdout OCR |",
            "| ---: | ---: | ---: | ---: | ---: | ---: |",
            f"| {first['access']['selected_tuning_pages']} | "
            f"{first['access']['tuning_assets_checksummed']} | "
            f"{first['access']['holdout_assets_resolved']} | "
            f"{first['access']['holdout_assets_checksummed']} | "
            f"{first['access']['holdout_assets_opened']} | "
            f"{first['access']['holdout_ocr_executions']} |",
            "",
            "## Aggregate measurements",
            "",
            "| Repetition | Candidate | CER | WER | Median s/page | p95 s/page | "
            "Peak RSS MiB | Failures |",
            "| ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: |",
        ]
    )
    for artifact in (first, second):
        for candidate in artifact["candidates"]:
            aggregate = candidate["aggregate"]
            lines.append(
                f"| {artifact['repetition']} | `{candidate['id']}` | "
                f"{aggregate['cer']:.6f} | {aggregate['wer']:.6f} | "
                f"{aggregate['latency_seconds']['median']:.6f} | "
                f"{aggregate['latency_seconds']['p95']:.6f} | "
                f"{aggregate['peak_rss_bytes'] / 1048576:.2f} | "
                f"{aggregate['failures']} |"
            )
    lines.extend(
        [
            "",
            "Timing varied as expected and was assessed separately:",
            "",
            "| Candidate | Minimum page seconds | Maximum page seconds | "
            "Combined median page seconds |",
            "| --- | ---: | ---: | ---: |",
        ]
    )
    for candidate_id, variance in comparison["timing_variance"].items():
        lines.append(
            f"| `{candidate_id}` | {variance['min_seconds']:.6f} | "
            f"{variance['max_seconds']:.6f} | "
            f"{variance['median_seconds']:.6f} |"
        )
    lines.extend(
        [
            "",
            "## Per-stratum aggregates",
            "",
            "Strata overlap by design. Values use the accepted raw counts from "
            "repetition 1; repetition 2 is count-identical.",
            "",
            "| Candidate | Stratum | Pages | Character edits / chars | CER | "
            "Word edits / words | WER |",
            "| --- | --- | ---: | ---: | ---: | ---: | ---: |",
        ]
    )
    for candidate in first["candidates"]:
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
            "## Raw additive counts",
            "",
            "These rows are sufficient to recompute every overall and overlapping-"
            "stratum micro-average. CER is total character edits divided by total "
            "chars; WER is total word edits divided by total words.",
            "",
            "| Candidate | Page ID | Difficulty strata | Character edits | Chars | "
            "Word edits | Words |",
            "| --- | --- | --- | ---: | ---: | ---: | ---: |",
        ]
    )
    for candidate in first["candidates"]:
        for record in candidate["records"]:
            strata = ", ".join(
                f"`{stratum}`" for stratum in record["difficulty_strata"]
            )
            lines.append(
                f"| `{candidate['id']}` | {record['page_id']} | "
                f"{strata} | "
                f"{record.get('character_edits', 0)} | "
                f"{record.get('reference_characters', 0)} | "
                f"{record.get('word_edits', 0)} | "
                f"{record.get('reference_words', 0)} |"
            )
    lines.extend(["", "Overall recomputation:"])
    for candidate in first["candidates"]:
        raw = candidate["aggregate"]["raw_counts"]
        lines.append(
            f"- `{candidate['id']}` CER: {raw['character_edits']} / "
            f"{raw['reference_characters']} = {candidate['aggregate']['cer']:.6f}; "
            f"WER: {raw['word_edits']} / {raw['reference_words']} = "
            f"{candidate['aggregate']['wer']:.6f}."
        )
    lines.extend(
        [
            "",
            "## Bounded execution semantics",
            "",
            "- Candidates ran serially, with one warm benchmark worker at a time "
            "and one fileconv process per page.",
            "- Each page had a 180-second wall deadline. Timeout cleanup terminates "
            "the complete process group.",
            "- Candidate stdout and stderr each had a hard 1,048,576-byte "
            "bounded-pipe collection limit; overflow terminates the process tree.",
            "- RSS is a 10 ms sampled process-tree sum. The 4 GiB threshold is a "
            "measured gate, not an OS-enforced memory limit.",
            "- Page latency spans warm worker request through result and includes "
            "the fileconv subprocess execution. Cold worker initialization is "
            "recorded separately in local run artifacts.",
            "- At most eight worst-CER page diagnostics per candidate are retained "
            "in local artifacts; diagnostics and tracked tables contain no OCR text.",
            "",
            "## Determinism",
            "",
            "The configured OCR-count tolerance is zero. Candidate order, argv, "
            "environment-variable names, source/split/config/binary/tessdata/host "
            "and toolchain checksums, all host/tool versions, holdout non-access "
            "evidence, per-page success state, and every additive edit count were "
            "identical. Only measured latency and RSS values were excluded from "
            "deterministic acceptance.",
            "",
        ]
    )
    return "\n".join(lines)


def _command_output(command: list[str]) -> str:
    result = subprocess.run(
        command,
        capture_output=True,
        text=True,
        check=True,
        timeout=30,
    )
    return (result.stdout or result.stderr).strip()


def _fileconv_package_version() -> str:
    manifest = tomllib.loads((ROOT / "crates" / "cli" / "Cargo.toml").read_text())
    package = manifest["package"]
    if "version" in package:
        return str(package["version"])
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())
    return str(workspace["workspace"]["package"]["version"])


def _toolchain_description() -> dict[str, Any]:
    return {
        "python": {
            "implementation": platform.python_implementation(),
            "version": platform.python_version(),
        },
        "tesseract": _command_output(["tesseract", "--version"]).splitlines(),
        "fileconv": {
            "package": "fileconv-cli",
            "version": _fileconv_package_version(),
        },
        "cargo": _command_output(["cargo", "--version"]),
        "rustc": _command_output(["rustc", "-Vv"]).splitlines(),
        "cc": {
            "tool": "gcc",
            "version": _command_output(["gcc", "--version"]).splitlines()[0],
        },
        "cxx": {
            "tool": "g++",
            "version": _command_output(["g++", "--version"]).splitlines()[0],
        },
        "build": {
            "profile": "release",
            "features": ["no-default-features"],
            "environment_variable_names": ["CC", "CXX"],
        },
    }


def _host_description(max_rss_bytes: int) -> dict[str, Any]:
    memory = psutil.virtual_memory()
    uname = platform.uname()
    return {
        "platform": platform.platform(),
        "system": uname.system,
        "kernel_release": uname.release,
        "kernel_version": uname.version,
        "architecture": platform.machine(),
        "logical_cpus": psutil.cpu_count(logical=True),
        "physical_cpus": psutil.cpu_count(logical=False),
        "memory_bytes": memory.total,
        "libc": list(platform.libc_ver()),
        "max_rss_bytes": max_rss_bytes,
        "max_rss_enforcement": "measured_gate_only_not_os_enforced",
    }


def run_matrix(args: argparse.Namespace) -> dict[str, Any]:
    config_path = args.config.resolve()
    config = load_baseline_config(config_path)
    if args.repetition not in range(1, config["repetitions"] + 1):
        raise ValueError("repetition is outside the frozen config")
    rows = load_accuracy_annotations(args.annotations.resolve())
    pages, split_payload = select_tuning_pages(
        rows,
        assets_dir=args.assets_dir.resolve(),
        expected_pages=config["expected_pages"],
    )
    fileconv = args.fileconv.resolve()
    auto_tessdata = resolve_auto_tessdata(
        cwd=Path.cwd(),
        executable=fileconv,
        manifest_dir=ROOT / "crates" / "core",
    )
    tessdata_paths = {
        "system-fast": args.system_tessdata.resolve(),
        "auto": auto_tessdata,
        "best": args.best_tessdata.resolve(),
    }
    host = _host_description(config["limits"]["max_rss_bytes"])
    toolchain = _toolchain_description()
    provenance = build_run_provenance(
        source_manifest=args.sources.resolve(),
        split_payload=split_payload,
        config_path=config_path,
        fileconv=fileconv,
        tessdata_paths=tessdata_paths,
        host=host,
        toolchain=toolchain,
    )
    specs, public = build_candidate_specs(
        config,
        fileconv=fileconv,
        system_tessdata=tessdata_paths["system-fast"],
        best_tessdata=tessdata_paths["best"],
        auto_tessdata=tessdata_paths["auto"],
        cpu_threads=config["limits"]["cpu_threads"],
        binary_sha256=provenance["binary_sha256"],
        tessdata_sha256=provenance["tessdata_sha256"],
        toolchain_sha256=provenance["toolchain_sha256"],
    )
    candidates = [
        _run_one_candidate(
            spec,
            public_candidate,
            pages,
            timeout_seconds=config["limits"]["timeout_seconds_per_page"],
            max_output_bytes=config["limits"]["max_output_bytes_per_stream"],
            max_rss_bytes=config["limits"]["max_rss_bytes"],
            diagnostic_limit=config["limits"]["diagnostic_pages_per_candidate"],
        )
        for spec, public_candidate in zip(specs, public, strict=True)
    ]
    artifact = {
        "schema_version": 1,
        "repetition": args.repetition,
        "split": "tuning",
        "page_count": len(pages),
        "provenance": provenance,
        "host": host,
        "toolchain": toolchain,
        "access": {
            "selected_tuning_pages": len(pages),
            "tuning_assets_checksummed": len(pages),
            "holdout_assets_resolved": 0,
            "holdout_assets_checksummed": 0,
            "holdout_assets_opened": 0,
            "holdout_ocr_executions": 0,
        },
        "candidates": candidates,
    }
    validate_run_artifact(artifact)
    return artifact


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--repetition", type=int)
    parser.add_argument(
        "--report-from",
        nargs=2,
        type=Path,
        metavar=("REPETITION_1", "REPETITION_2"),
    )
    parser.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
    parser.add_argument("--annotations", type=Path, default=DEFAULT_ANNOTATIONS)
    parser.add_argument("--sources", type=Path, default=DEFAULT_SOURCES)
    parser.add_argument("--assets-dir", type=Path, default=DEFAULT_ASSETS)
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
    return parser


def main() -> None:
    args = _parser().parse_args()
    if args.report_from:
        first, second = (
            json.loads(path.read_text(encoding="utf-8"))
            for path in args.report_from
        )
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(
            render_baseline_report(first, second),
            encoding="utf-8",
        )
        return
    if args.repetition is None:
        raise SystemExit("--repetition is required unless --report-from is used")
    artifact = run_matrix(args)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps(artifact, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )


if __name__ == "__main__":
    main()
