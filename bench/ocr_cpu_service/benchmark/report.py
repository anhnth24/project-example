"""Gate evaluation and deterministic Markdown rendering for Phase A."""

from __future__ import annotations

import argparse
import json
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


def _rate(value: float) -> str:
    return f"{value:.4f}"


def _mib(value: int) -> str:
    return f"{value / (1024 * 1024):.1f}"


def render_markdown(data: dict[str, Any]) -> str:
    """Render a metadata-and-metrics-only report from raw summary JSON."""
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
        "## Candidate summary",
        "",
        "| Candidate | CER | WER | Median s/page | p95 s/page | Peak RSS MiB | Failures |",
        "|---|--:|--:|--:|--:|--:|--:|",
    ]
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
            "| Candidate | Source ID | Stratum | Page | CER | WER | Seconds | Peak RSS MiB | Status |",
            "|---|---|---|--:|--:|--:|--:|--:|---|",
        ]
    )
    for candidate in data["candidates"]:
        for page in candidate.get("pages", []):
            if page["stratum"] == "official-government":
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
            f"- Inspection: {evidence['pages']} pages; "
            f"{evidence['text_pages']} with extractable text; "
            f"{evidence['image_pages']} with page images.",
            "- Deterministic sampled pages: "
            + ", ".join(str(page) for page in official["sampled_pages"])
            + ".",
            "- This source has no pinned human-verified page transcription and "
            "is excluded from CER/WER and the quality gate.",
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
    output = args.output or args.summary.with_suffix(".md")
    output.write_text(render_markdown(data), encoding="utf-8")


if __name__ == "__main__":
    main()
