from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parents[1]))

from benchmark.report import evaluate_gate, render_markdown  # noqa: E402
from benchmark.run import (  # noqa: E402
    BenchmarkPage,
    _run_candidate,
    aggregate_records,
    deterministic_page_sample,
)


def test_gate_uses_better_tesseract_baseline() -> None:
    result = evaluate_gate(default_cer=0.20, best_cer=0.10, paddle_cer=0.07)

    assert result.baseline_cer == 0.10
    assert result.relative_improvement == pytest.approx(0.30)
    assert result.passed


def test_gate_fails_at_less_than_twenty_percent() -> None:
    assert not evaluate_gate(0.20, 0.10, 0.081).passed


def test_gate_enforces_stratum_and_completion_requirements() -> None:
    regression = evaluate_gate(
        0.20,
        0.10,
        0.07,
        baseline_cer_by_stratum={"real-scan": 0.10, "synthetic-scan": 0.02},
        paddle_cer_by_stratum={"real-scan": 0.07, "synthetic-scan": 0.071},
    )
    failure = evaluate_gate(0.20, 0.10, 0.07, failures=1)
    resource = evaluate_gate(
        0.20, 0.10, 0.07, resource_limit_violations=1
    )

    assert not regression.passed
    assert "synthetic-scan" in " ".join(regression.reasons)
    assert not failure.passed
    assert not resource.passed


def test_markdown_is_deterministic_metadata_only_rendering() -> None:
    data = {
        "schema_version": 1,
        "generated_at_utc": "2026-08-07T00:00:00Z",
        "run": {
            "commit": "abc123",
            "host": {"logical_cpus": 8, "memory_bytes": 1024},
            "versions": {"python": "3.12"},
        },
        "corpus": {
            "manifest_sha256": "a" * 64,
            "quantitative_pages": 12,
            "strata": {"real-scan": 9, "synthetic-scan": 3},
            "official_sample": {
                "source_id": "official-89-2026-tt-btc",
                "classification": "scan",
                "classification_evidence": {
                    "pages": 839,
                    "text_pages": 0,
                    "image_pages": 839,
                },
                "sampled_pages": [1, 420, 839],
            },
        },
        "candidates": [
            {
                "id": "markhand-default",
                "label": "Markhand default",
                "aggregate": {
                    "cer": 0.2,
                    "wer": 0.3,
                    "median_seconds_per_page": 1.0,
                    "p95_seconds_per_page": 1.2,
                    "peak_rss_bytes": 100,
                    "failures": 0,
                },
                "strata": {"real-scan": {"cer": 0.2, "wer": 0.3}},
                "pages": [
                    {
                        "source_id": "sample-1",
                        "stratum": "real-scan",
                        "page_number": 1,
                        "success": True,
                        "cer": 0.2,
                        "wer": 0.3,
                        "elapsed_seconds": 1.0,
                        "peak_rss_bytes": 100,
                    }
                ],
            }
        ],
        "gate": {
            "decision": "STOP",
            "baseline_cer": 0.1,
            "paddle_cer": 0.09,
            "relative_improvement": 0.1,
            "threshold": 0.2,
            "reasons": ["relative CER improvement below 20%"],
        },
    }

    first = render_markdown(data)
    second = render_markdown(json.loads(json.dumps(data)))

    assert first == second
    assert "# Phase A CPU OCR benchmark" in first
    assert "**STOP**" in first
    assert "1, 420, 839" in first
    assert "## Per-page quantitative metrics" in first
    assert "sample-1" in first
    assert "complete OCR output" not in first


def test_official_sample_is_bounded_and_deterministic() -> None:
    assert deterministic_page_sample(839) == (1, 420, 839)
    assert deterministic_page_sample(2) == (1, 2)


def test_aggregate_uses_micro_error_rates_and_latency_percentiles() -> None:
    records = [
        {
            "success": True,
            "character_edits": 1,
            "reference_characters": 10,
            "word_edits": 1,
            "reference_words": 5,
            "elapsed_seconds": 1.0,
            "peak_rss_bytes": 100,
        },
        {
            "success": True,
            "character_edits": 9,
            "reference_characters": 90,
            "word_edits": 3,
            "reference_words": 15,
            "elapsed_seconds": 3.0,
            "peak_rss_bytes": 200,
        },
    ]

    aggregate = aggregate_records(records)

    assert aggregate["cer"] == pytest.approx(0.10)
    assert aggregate["wer"] == pytest.approx(0.20)
    assert aggregate["median_seconds_per_page"] == pytest.approx(2.0)
    assert aggregate["p95_seconds_per_page"] == pytest.approx(2.9)
    assert aggregate["peak_rss_bytes"] == 200
    assert aggregate["failures"] == 0


def test_candidate_summary_counts_failed_quantitative_pages(
    tmp_path: Path,
) -> None:
    class FailingCandidate:
        id = "candidate"
        label = "Candidate"
        metadata: dict[str, object] = {}

        def recognize(
            self, page: BenchmarkPage
        ) -> tuple[str, float, int]:
            if page.source_id == "failed":
                raise RuntimeError("failure")
            return "reference", 1.0, 100

    pages = [
        BenchmarkPage(
            source_id=source_id,
            source_sha256="a" * 64,
            stratum="real-scan",
            page_number=1,
            path=tmp_path / source_id,
            reference="reference",
        )
        for source_id in ("ok", "failed")
    ]

    result = _run_candidate(
        FailingCandidate(), pages, max_rss_bytes=1000
    )

    assert result["aggregate"]["pages"] == 2
    assert result["aggregate"]["failures"] == 1
    assert result["strata"]["real-scan"]["failures"] == 1
