"""Gate evaluation and deterministic Markdown rendering for Phase A."""

from __future__ import annotations

import argparse
import copy
import json
import statistics
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

GATE_THRESHOLD = 0.20
MAX_STRATUM_REGRESSION = 0.05


@dataclass(frozen=True, slots=True)
class GateResult:
    passed: bool
    baseline_cer: float
    paddle_cer: float
    relative_improvement: float
    threshold: float
    reasons: tuple[str, ...]

    def as_json(self) -> dict[str, Any]:
        result = asdict(self)
        result["decision"] = "PASS" if self.passed else "STOP"
        result["reasons"] = list(self.reasons)
        return result


def evaluate_gate(
    default_cer: float,
    best_cer: float,
    paddle_cer: float,
    *,
    baseline_cer_by_stratum: dict[str, float] | None = None,
    paddle_cer_by_stratum: dict[str, float] | None = None,
    failures: int = 0,
    resource_limit_violations: int = 0,
) -> GateResult:
    """Evaluate all Phase A criteria against measured values."""
    baseline_cer = min(default_cer, best_cer)
    relative_improvement = (
        (baseline_cer - paddle_cer) / baseline_cer
        if baseline_cer > 0
        else 0.0
    )
    reasons: list[str] = []
    if relative_improvement < GATE_THRESHOLD:
        reasons.append("relative real-scan CER improvement below 20%")

    baseline_strata = baseline_cer_by_stratum or {}
    paddle_strata = paddle_cer_by_stratum or {}
    for stratum in sorted(set(baseline_strata) | set(paddle_strata)):
        if stratum not in baseline_strata or stratum not in paddle_strata:
            reasons.append(f"{stratum}: missing comparable stratum result")
        elif paddle_strata[stratum] - baseline_strata[stratum] > (
            MAX_STRATUM_REGRESSION
        ):
            reasons.append(f"{stratum}: CER regression exceeds 0.05")

    if failures:
        reasons.append(f"{failures} benchmark page failure(s)")
    if resource_limit_violations:
        reasons.append(
            f"{resource_limit_violations} resource-limit violation(s)"
        )
    return GateResult(
        passed=not reasons,
        baseline_cer=baseline_cer,
        paddle_cer=paddle_cer,
        relative_improvement=relative_improvement,
        threshold=GATE_THRESHOLD,
        reasons=tuple(reasons),
    )


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
    """Micro-average raw additive counts and measured warm page resources."""
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


def _gate_records(candidate: dict[str, Any]) -> list[dict[str, Any]]:
    return [
        record
        for record in candidate["pages"]
        if record.get(
            "gate_included",
            record["stratum"] in {"real-scan", "synthetic-scan"},
        )
    ]


def recompute_and_validate_summary(data: dict[str, Any]) -> dict[str, Any]:
    """Recompute every derived value and reject contradictory stored claims."""
    result = copy.deepcopy(data)
    for candidate in result["candidates"]:
        records = _gate_records(candidate)
        strata = {
            stratum: aggregate_records(
                [record for record in records if record["stratum"] == stratum]
            )
            for stratum in sorted({record["stratum"] for record in records})
        }
        aggregate = aggregate_records(records)
        if "aggregate" in candidate and candidate["aggregate"] != aggregate:
            raise ValueError(
                f"{candidate['id']}: stored aggregate contradicts raw page counts"
            )
        if "strata" in candidate and candidate["strata"] != strata:
            raise ValueError(
                f"{candidate['id']}: stored strata contradict raw page counts"
            )
        candidate["aggregate"] = aggregate
        candidate["strata"] = strata

    by_id = {candidate["id"]: candidate for candidate in result["candidates"]}
    required = {
        "markhand-default",
        "markhand-tessdata-best",
        "pp-ocrv6",
    }
    if set(by_id) != required:
        raise ValueError("summary must contain exactly the three Phase A candidates")
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
        not record["success"]
        for candidate in result["candidates"]
        for record in candidate["pages"]
    )
    resource_violations = sum(
        record.get("resource_limit_violation", False)
        for candidate in result["candidates"]
        for record in candidate["pages"]
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
    ).as_json()
    if "gate" in result and result["gate"] != gate:
        raise ValueError("stored gate contradicts raw page counts")
    result["gate"] = gate
    return result


def _rate(value: float) -> str:
    return f"{value:.4f}"


def _mib(value: int) -> str:
    return f"{value / (1024 * 1024):.1f}"


def render_markdown(data: dict[str, Any]) -> str:
    """Render a metadata-and-metrics-only report from raw summary JSON."""
    data = recompute_and_validate_summary(data)
    gate = data["gate"]
    corpus = data["corpus"]
    run = data["run"]
    official = corpus["official_sample"]
    lines = [
        "# Phase A CPU OCR benchmark",
        "",
        f"Gate decision: **{gate['decision']}**",
        "",
        "This report contains metrics and metadata only; no complete document "
        "text or OCR output is included.",
        "",
        "## Run metadata",
        "",
        f"- Generated (UTC): `{data['generated_at_utc']}`",
        f"- Commit: `{run['commit']}`",
        f"- Host: {run['host']['logical_cpus']} logical CPUs, "
        f"{_mib(run['host']['memory_bytes'])} MiB RAM",
        f"- Corpus manifest SHA-256: `{corpus['manifest_sha256']}`",
        f"- Quantitative pages: {corpus['quantitative_pages']}",
        "- Strata: "
        + ", ".join(
            f"{name}={count}"
            for name, count in sorted(corpus["strata"].items())
        ),
        "",
        "## Sample-size and representativeness limits",
        "",
        f"- Only {corpus['strata'].get('real-scan', 0)} real-scan pages and "
        f"{corpus['strata'].get('synthetic-scan', 0)} synthetic-scan pages "
        "have pinned human-verified text.",
        "- These descriptive CER/WER values apply to this bounded pinned "
        "sample; they are not a population estimate and no confidence "
        "interval is claimed.",
        "- The synthetic receipt stratum is reported separately and must not "
        "be treated as additional real-document evidence.",
        "- The official mixed PDF sample has no human-verified transcription, "
        "so it contributes runtime/failure context only and cannot reduce "
        "quality uncertainty.",
        "",
        "## Cold initialization and resource semantics",
        "",
        "Cold initialization is measured once from isolated worker process "
        "start through its ready event. Per-page latency is warm worker-request "
        "wall time and excludes that cold start. RSS values are 10 ms sampled "
        "process-tree RSS sums during each labeled interval; no before/after "
        "value is labeled as peak.",
        "",
        "| Candidate | Cold wall seconds | Cold candidate seconds | Cold sampled process-tree RSS MiB |",
        "|---|--:|--:|--:|",
    ]
    for candidate in data["candidates"]:
        cold = candidate.get("metadata", {}).get("cold_initialization")
        if cold:
            lines.append(
                f"| {candidate['label']} | {cold['wall_seconds']:.3f} | "
                f"{cold['candidate_seconds']:.3f} | "
                f"{_mib(cold['rss_measurement']['peak_rss_bytes'])} |"
            )
        else:
            lines.append(f"| {candidate['label']} | — | — | — |")
    lines.extend(
        [
            "",
            "## Candidate environment and build provenance",
            "",
        ]
    )
    for candidate in data["candidates"]:
        environment = candidate.get("metadata", {}).get("environment", {})
        rendered_environment = ", ".join(
            f"{key}={value}" for key, value in sorted(environment.items())
        )
        lines.append(
            f"- {candidate['label']} sanitized environment: "
            f"`{rendered_environment or 'not recorded'}`."
        )
    fileconv_build = next(
        (
            candidate.get("metadata", {}).get("fileconv_build")
            for candidate in data["candidates"]
            if candidate.get("metadata", {}).get("fileconv_build")
        ),
        None,
    )
    if fileconv_build:
        lines.extend(
            [
                f"- fileconv binary SHA-256: "
                f"`{fileconv_build['binary_sha256']}`.",
                f"- fileconv build command: "
                f"`{fileconv_build['build_command']}`.",
                "- fileconv build features: `"
                + ", ".join(fileconv_build["build_features"])
                + "`; profile: `"
                + fileconv_build["profile"]
                + "`.",
            ]
        )
    lines.extend(
        [
        "",
        "## Candidate summary",
        "",
        "| Candidate | CER | WER | Warm median s/page | Warm p95 s/page | Warm sampled RSS MiB | Failures |",
        "|---|--:|--:|--:|--:|--:|--:|",
        ]
    )
    for candidate in data["candidates"]:
        aggregate = candidate["aggregate"]
        lines.append(
            f"| {candidate['label']} | {_rate(aggregate['cer'])} | "
            f"{_rate(aggregate['wer'])} | "
            f"{aggregate['median_seconds_per_page']:.3f} | "
            f"{aggregate['p95_seconds_per_page']:.3f} | "
            f"{_mib(aggregate['peak_rss_bytes'])} | "
            f"{aggregate['failures']} |"
        )

    lines.extend(
        [
            "",
            "## CER/WER by stratum",
            "",
            "| Candidate | Stratum | Pages | CER | WER |",
            "|---|---|--:|--:|--:|",
        ]
    )
    for candidate in data["candidates"]:
        for stratum, metrics in sorted(candidate["strata"].items()):
            lines.append(
                f"| {candidate['label']} | {stratum} | "
                f"{metrics.get('pages', '—')} | {_rate(metrics['cer'])} | "
                f"{_rate(metrics['wer'])} |"
            )

    lines.extend(
        [
            "",
            "## Per-page quantitative metrics",
            "",
            "| Candidate | Source ID | Stratum | Page | CER | WER | Warm seconds | Sampled process-tree RSS MiB | Status |",
            "|---|---|---|--:|--:|--:|--:|--:|---|",
        ]
    )
    for candidate in data["candidates"]:
        for page in candidate.get("pages", []):
            if not page.get(
                "gate_included",
                page["stratum"] in {"real-scan", "synthetic-scan"},
            ):
                continue
            status = "ok" if page["success"] else page.get("error_kind", "failed")
            lines.append(
                f"| {candidate['label']} | `{page['source_id']}` | "
                f"{page['stratum']} | {page['page_number']} | "
                f"{_rate(page.get('cer', 0.0)) if page['success'] else '—'} | "
                f"{_rate(page.get('wer', 0.0)) if page['success'] else '—'} | "
                f"{page['elapsed_seconds']:.3f} | "
                f"{_mib(page['peak_rss_bytes'])} | {status} |"
            )

    evidence = official["classification_evidence"]
    lines.extend(
        [
            "",
            "## Official 89/2026/TT-BTC bounded sample",
            "",
            f"- Classification: **{official['classification']}**.",
            "- Benchmark stratum: **mixed**; gate-included: **false**.",
            "- Manifest/inspection mismatch: "
            f"**{str(official.get('classification_mismatch', False)).lower()}**.",
            f"- Inspection: {evidence['pages']} pages; "
            f"{evidence['text_pages']} with extractable text; "
            f"{evidence['image_pages']} with page images.",
            "- Deterministic sampled pages: "
            + ", ".join(str(page) for page in official["sampled_pages"])
            + ".",
            "- This source has no pinned human-verified page transcription and "
            "is excluded from CER/WER and the quality gate.",
            "",
            "## Official sample runtime evidence",
            "",
            "| Candidate | Page | Warm seconds | Sampled process-tree RSS MiB | Status |",
            "|---|--:|--:|--:|---|",
        ]
    )
    for candidate in data["candidates"]:
        for page in candidate.get("pages", []):
            if page.get(
                "gate_included",
                page["stratum"] in {"real-scan", "synthetic-scan"},
            ):
                continue
            status = "ok" if page["success"] else page.get("error_kind", "failed")
            lines.append(
                f"| {candidate['label']} | {page['page_number']} | "
                f"{page['elapsed_seconds']:.3f} | "
                f"{_mib(page['peak_rss_bytes'])} | {status} |"
            )

    lines.extend(
        [
            "",
            "## Gate",
            "",
            f"- Better Tesseract real-scan CER: {_rate(gate['baseline_cer'])}",
            f"- PP-OCRv6 real-scan CER: {_rate(gate['paddle_cer'])}",
            "- Relative improvement: "
            f"{gate['relative_improvement'] * 100:.2f}% "
            f"(required: {gate['threshold'] * 100:.0f}%)",
        ]
    )
    reasons = gate.get("reasons", [])
    if reasons:
        lines.append("- Decision reasons:")
        lines.extend(f"  - {reason}" for reason in reasons)
    else:
        lines.append("- All Phase A criteria passed.")

    lines.extend(["", "## Tool versions", ""])
    lines.extend(
        f"- {name}: `{version}`"
        for name, version in sorted(run["versions"].items())
    )
    return "\n".join(lines) + "\n"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("summary", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    data = json.loads(args.summary.read_text(encoding="utf-8"))
    data = recompute_and_validate_summary(data)
    output = args.output or args.summary.with_suffix(".md")
    output.write_text(render_markdown(data), encoding="utf-8")


if __name__ == "__main__":
    main()
