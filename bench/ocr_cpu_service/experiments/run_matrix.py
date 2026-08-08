"""Deterministic, tuning-only OCR baseline experiment runner."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import platform
import statistics
import subprocess
from dataclasses import asdict
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable

import psutil

from benchmark.candidates import CommandCandidateSpec
from benchmark.corpus import BenchmarkPage
from benchmark.metrics import error_counts
from benchmark.run import (
    FILECONV_BUILD_COMMAND,
    CandidateOutputLimitError,
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
_CHECKSUM_FIELDS = frozenset(
    {
        "source_sha256",
        "split_sha256",
        "config_sha256",
        "binary_sha256",
        "tessdata_sha256",
        "host_sha256",
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
    }


def build_candidate_specs(
    config: dict[str, Any],
    *,
    fileconv: Path,
    system_tessdata: Path,
    best_tessdata: Path,
    cpu_threads: int,
    binary_sha256: str,
    tessdata_sha256: dict[str, dict[str, str]],
) -> tuple[list[CommandCandidateSpec], list[dict[str, Any]]]:
    base_environment = sanitized_candidate_environment(cpu_threads=cpu_threads)
    substitutions = {
        "{fileconv}": str(fileconv),
        "{system_tessdata}": str(system_tessdata),
        "{best_tessdata}": str(best_tessdata),
    }
    specs: list[CommandCandidateSpec] = []
    public: list[dict[str, Any]] = []
    for candidate in config["candidates"]:
        argv = tuple(substitutions.get(value, value) for value in candidate["argv"])
        candidate_environment = {
            name: substitutions[value]
            for name, value in candidate["environment"].items()
        }
        environment = {**base_environment, **candidate_environment}
        role = (
            "best"
            if candidate_environment["FILECONV_TESSDATA"] == str(best_tessdata)
            else "system"
        )
        provenance = {
            "binary_sha256": binary_sha256,
            "tessdata_role": role,
            "tessdata_sha256": tessdata_sha256[role],
            "build_command": FILECONV_BUILD_COMMAND,
            "build_features": ["no-default-features"],
            "profile": "release",
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
    if artifact.get("split") != "tuning":
        raise ValueError("run artifact must contain tuning only")
    provenance = artifact.get("provenance")
    if not isinstance(provenance, dict) or set(provenance) != _CHECKSUM_FIELDS:
        raise ValueError("run provenance checksum set is incomplete")
    if not all(_valid_checksum_value(value) for value in provenance.values()):
        raise ValueError("run provenance contains an invalid checksum")
    _assert_no_recognized_text(artifact)
    for candidate in artifact.get("candidates", []):
        if "environment" in candidate:
            raise ValueError("candidate environment values must not be stored")
        records = candidate["records"]
        if len(candidate["diagnostics"]) > 8:
            raise ValueError("bounded page diagnostics limit exceeded")
        if candidate["aggregate"] != aggregate_records(records):
            raise ValueError("candidate aggregate does not match raw records")
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


def _deterministic_projection(artifact: dict[str, Any]) -> dict[str, Any]:
    return {
        "split": artifact["split"],
        "page_count": artifact["page_count"],
        "provenance": artifact["provenance"],
        "candidates": [
            {
                "id": candidate["id"],
                "argv": candidate["argv"],
                "environment_variable_names": candidate[
                    "environment_variable_names"
                ],
                "provenance": candidate["provenance"],
                "records": [
                    {
                        "page_id": record["page_id"],
                        "success": record["success"],
                        "error_kind": record["error_kind"],
                        **{
                            field: record.get(field)
                            for field in _COUNT_FIELDS
                        },
                    }
                    for record in candidate["records"]
                ],
            }
            for candidate in artifact["candidates"]
        ],
    }


def compare_repetitions(
    first: dict[str, Any], second: dict[str, Any]
) -> dict[str, Any]:
    validate_run_artifact(first)
    validate_run_artifact(second)
    if first["provenance"] != second["provenance"]:
        raise ValueError("candidate or run provenance changed between repetitions")
    left = _deterministic_projection(first)
    right = _deterministic_projection(second)
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
                f"- Memory bytes: {first['host']['memory_bytes']}",
                f"- Tesseract: `{first['versions']['tesseract']}`",
                f"- Python: `{first['versions']['python']}`",
                f"- Build: `{first['candidates'][0]['provenance']['build_command']}`",
            ]
        )
    lines.extend(
        [
            "",
            "## Candidate interfaces",
            "",
            "| Candidate | Exact argv | Environment variable names |",
            "| --- | --- | --- |",
        ]
    )
    for candidate in first["candidates"]:
        argv = json.dumps(candidate["argv"], ensure_ascii=False)
        names = ", ".join(
            f"`{name}`" for name in candidate["environment_variable_names"]
        )
        lines.append(f"| `{candidate['id']}` | `{argv}` | {names} |")
    lines.extend(
        [
            "",
            "Only variable names are recorded; environment values are omitted.",
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
            "checksums, per-page success state, and every additive edit count were "
            "identical. Latency and RSS variance did not participate in acceptance.",
            "",
        ]
    )
    return "\n".join(lines)


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
    tessdata_paths = {
        "system": args.system_tessdata.resolve(),
        "best": args.best_tessdata.resolve(),
    }
    host = _host_description(config["limits"]["max_rss_bytes"])
    provenance = build_run_provenance(
        source_manifest=args.sources.resolve(),
        split_payload=split_payload,
        config_path=config_path,
        fileconv=fileconv,
        tessdata_paths=tessdata_paths,
        host=host,
    )
    specs, public = build_candidate_specs(
        config,
        fileconv=fileconv,
        system_tessdata=tessdata_paths["system"],
        best_tessdata=tessdata_paths["best"],
        cpu_threads=config["limits"]["cpu_threads"],
        binary_sha256=provenance["binary_sha256"],
        tessdata_sha256=provenance["tessdata_sha256"],
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
        "generated_at_utc": datetime.now(timezone.utc)
        .isoformat(timespec="seconds")
        .replace("+00:00", "Z"),
        "repetition": args.repetition,
        "split": "tuning",
        "page_count": len(pages),
        "provenance": provenance,
        "host": host,
        "versions": {
            "python": platform.python_version(),
            "tesseract": subprocess.run(
                ["tesseract", "--version"],
                capture_output=True,
                text=True,
                check=True,
                timeout=30,
            ).stdout.splitlines()[0],
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
