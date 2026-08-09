from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parents[1]))

from benchmark.report import (  # noqa: E402
    aggregate_records,
    evaluate_gate,
    recompute_and_validate_summary,
    render_markdown,
)
from benchmark.corpus import (  # noqa: E402
    BenchmarkPage,
    deterministic_page_sample,
)
from benchmark.run import (  # noqa: E402
    IsolatedCandidateWorker,
    RecognitionMeasurement,
    _fileconv_provenance,
    _read_event_with_process_tree_rss,
    _run_candidate,
    sanitized_candidate_environment,
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


def test_report_recomputes_gate_and_rejects_inconsistent_stored_summary() -> None:
    def candidate(candidate_id: str, edits: int) -> dict[str, object]:
        return {
            "id": candidate_id,
            "label": candidate_id,
            "metadata": {},
            "pages": [
                {
                    "source_id": "page",
                    "stratum": "real-scan",
                    "page_number": 1,
                    "success": True,
                    "character_edits": edits,
                    "reference_characters": 10,
                    "word_edits": edits,
                    "reference_words": 5,
                    "elapsed_seconds": 1.0,
                    "peak_rss_bytes": 100,
                    "resource_limit_violation": False,
                }
            ],
        }

    raw = {
        "schema_version": 1,
        "run": {
            "versions": {
                "paddleocr": "archived",
                "paddlepaddle": "archived",
                "paddlex": "archived",
            }
        },
        "candidates": [
            candidate("markhand-default", 2),
            candidate("markhand-tessdata-best", 1),
            candidate("pp-ocrv6", 0),
        ],
        "gate": {
            "passed": True,
            "baseline_cer": 0.1,
            "paddle_cer": 0.0,
            "relative_improvement": 1.0,
            "threshold": 0.2,
            "reasons": [],
            "decision": "PASS",
        },
    }

    computed = recompute_and_validate_summary(raw)

    assert computed["candidates"][0]["aggregate"]["cer"] == 0.2
    assert computed["gate"]["decision"] == "PASS"
    computed["candidates"][0]["aggregate"]["cer"] = 0.9
    with pytest.raises(ValueError, match="stored aggregate"):
        recompute_and_validate_summary(computed)


def _generic_report_payload(
    *,
    comparison: dict[str, object] | None,
) -> dict[str, object]:
    def candidate(candidate_id: str, edits: int) -> dict[str, object]:
        return {
            "id": candidate_id,
            "label": candidate_id,
            "metadata": {},
            "pages": [
                {
                    "source_id": "page",
                    "stratum": "real-scan",
                    "page_number": 1,
                    "success": True,
                    "character_edits": edits,
                    "reference_characters": 10,
                    "word_edits": edits,
                    "reference_words": 5,
                    "elapsed_seconds": 1.0,
                    "peak_rss_bytes": 100,
                    "resource_limit_violation": False,
                }
            ],
        }

    payload: dict[str, object] = {
        "schema_version": 1,
        "generated_at_utc": "2026-08-08T00:00:00Z",
        "run": {
            "commit": "abc123",
            "host": {"logical_cpus": 2, "memory_bytes": 1024},
            "versions": {"python": "3.12"},
        },
        "corpus": {
            "manifest_sha256": "a" * 64,
            "quantitative_pages": 1,
            "strata": {"real-scan": 1},
            "official_sample": {
                "source_id": "official",
                "classification": "scan",
                "classification_evidence": {
                    "pages": 1,
                    "text_pages": 0,
                    "image_pages": 1,
                },
                "sampled_pages": [1],
            },
            "historical_samples": [],
            "reading_order_cases": [],
        },
        "candidates": [
            candidate("control-a", 2),
            candidate("challenger-z", 1),
        ],
    }
    if comparison is not None:
        payload["comparison"] = comparison
    return payload


def test_report_uses_explicit_comparison_roles_not_candidate_names() -> None:
    payload = _generic_report_payload(
        comparison={"baseline": "control-a", "challenger": "challenger-z"}
    )

    markdown = render_markdown(payload)

    assert "control-a" in markdown
    assert "challenger-z" in markdown
    assert "Challenger real-scan CER" in markdown
    assert "PP-OCRv6 real-scan CER" not in markdown


def test_report_without_challenger_states_no_adoption_gate() -> None:
    payload = _generic_report_payload(comparison=None)

    markdown = render_markdown(payload)

    assert "no adoption gate was configured" in markdown.lower()
    assert "Gate decision:" not in markdown
    assert "measured gate, not an OS-enforced limit" in markdown
    assert "stdout and stderr are hard bounded" in markdown


def test_generic_report_lists_environment_names_without_values() -> None:
    canary = "REPORT_CANARY_MUST_NOT_ESCAPE"
    payload = _generic_report_payload(comparison=None)
    candidates = payload["candidates"]
    assert isinstance(candidates, list)
    for candidate in candidates:
        candidate["metadata"] = {
            "environment": {"OCR_CANARY_SECRET": canary},
            "environment_variable_names": ["LANG", "OCR_CANARY_SECRET"],
        }

    markdown = render_markdown(payload)

    assert "OCR_CANARY_SECRET" in markdown
    assert canary not in markdown
    assert "OCR_CANARY_SECRET=" not in markdown


def test_same_candidate_ids_without_archived_metadata_remain_generic() -> None:
    payload = _generic_report_payload(comparison=None)
    candidates = payload["candidates"]
    assert isinstance(candidates, list)
    candidates[0]["id"] = "markhand-default"
    candidates[0]["label"] = "Markhand default"
    candidates[1]["id"] = "markhand-tessdata-best"
    candidates[1]["label"] = "Markhand tessdata_best"
    paddle_named_only = json.loads(json.dumps(candidates[0]))
    paddle_named_only["id"] = "pp-ocrv6"
    paddle_named_only["label"] = "PP-OCRv6"
    candidates.append(paddle_named_only)

    markdown = render_markdown(payload)

    assert "no adoption gate was configured" in markdown.lower()
    assert "Gate decision:" not in markdown
    assert "PP-OCRv6 real-scan CER" not in markdown


def test_markdown_is_deterministic_metadata_only_rendering() -> None:
    data = {
        "schema_version": 1,
        "generated_at_utc": "2026-08-07T00:00:00Z",
        "run": {
            "commit": "abc123",
            "host": {"logical_cpus": 8, "memory_bytes": 1024},
            "versions": {
                "paddleocr": "archived",
                "paddlepaddle": "archived",
                "paddlex": "archived",
                "python": "3.12",
            },
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
            "historical_samples": [
                {
                    "source_id": "wikimedia-history",
                    "classification": "scan",
                    "sampled_pages": [1],
                    "evidence_mode": "qualitative-only",
                    "transcription": "none-trustworthy-available",
                    "reading_order_review": {
                        "page_number": 1,
                        "review_status": "human-reviewed-short-anchors",
                        "expected_sequence": ["MỤC TRÁI", "MỤC PHẢI"],
                    },
                }
            ],
            "reading_order_cases": [
                {
                    "source_id": "reviewed-multicolumn-v1",
                    "classification": "synthetic-scan",
                    "ground_truth": "deterministic-source",
                    "expected_anchors": 6,
                    "page_number": 1,
                    "expected_sequence": ["L1", "L2", "L3", "R1", "R2", "R3"],
                },
                {
                    "source_id": "wikimedia-history",
                    "classification": "scan",
                    "ground_truth": "human-reviewed-short-anchors",
                    "expected_anchors": 2,
                    "page_number": 1,
                    "expected_sequence": ["MỤC TRÁI", "MỤC PHẢI"],
                }
            ],
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
                    },
                    {
                        "source_id": "official-89-2026-tt-btc",
                        "stratum": "mixed",
                        "gate_included": False,
                        "page_number": 420,
                        "success": True,
                        "elapsed_seconds": 2.5,
                        "peak_rss_bytes": 200,
                    },
                    {
                        "source_id": "wikimedia-history",
                        "stratum": "historical-scan",
                        "gate_included": False,
                        "page_number": 1,
                        "success": True,
                        "elapsed_seconds": 3.0,
                        "peak_rss_bytes": 220,
                        "reading_order": {
                            "expected_anchors": 2,
                            "observed_anchors": 2,
                            "comparable_pairs": 1,
                            "violations": 0,
                            "missing_anchors": 0,
                        },
                    },
                    {
                        "source_id": "reviewed-multicolumn-v1",
                        "stratum": "reviewed-multicolumn",
                        "gate_included": False,
                        "page_number": 1,
                        "success": True,
                        "elapsed_seconds": 1.5,
                        "peak_rss_bytes": 180,
                        "reading_order": {
                            "expected_anchors": 6,
                            "observed_anchors": 6,
                            "comparable_pairs": 15,
                            "violations": 1,
                            "missing_anchors": 0,
                        },
                    }
                ],
            }
        ],
        "gate": {
            "passed": False,
            "decision": "STOP",
            "baseline_cer": 0.1,
            "paddle_cer": 0.9,
            "relative_improvement": -8.0,
            "threshold": 0.2,
            "reasons": [
                "relative real-scan CER improvement below 20%",
                "real-scan: CER regression exceeds 0.05",
            ],
        },
    }
    quantitative_page = data["candidates"][0]["pages"][0]
    qualitative_pages = data["candidates"][0]["pages"][1:]
    data["candidates"] = [
        {
            "id": candidate_id,
            "label": label,
            "metadata": {},
            "pages": [
                {
                    **quantitative_page,
                    "character_edits": edits,
                    "reference_characters": 10,
                    "word_edits": edits,
                    "reference_words": 5,
                    "resource_limit_violation": False,
                },
                *[
                    {**page, "resource_limit_violation": False}
                    for page in qualitative_pages
                ],
            ],
        }
        for candidate_id, label, edits in (
            ("markhand-default", "Markhand default", 2),
            ("markhand-tessdata-best", "Markhand tessdata_best", 1),
            ("pp-ocrv6", "PP-OCRv6", 9),
        )
    ]
    for candidate in data["candidates"]:
        candidate["metadata"]["environment"] = {
            "PATH": "/usr/local/bin:/usr/bin:/bin",
            "PYTHONNOUSERSITE": "1",
        }
        candidate["pages"].append(
            {
                "source_id": "wikimedia-history",
                "stratum": "historical-scan",
                "gate_included": False,
                "page_number": 2,
                "success": True,
                "elapsed_seconds": 2.0,
                "peak_rss_bytes": 200,
                "resource_limit_violation": False,
            }
        )
    for candidate in data["candidates"][:2]:
        candidate["metadata"]["timing_note"] = (
            "warm timing includes a fresh fileconv/Tesseract subprocess"
        )
        candidate["metadata"]["fileconv_build"] = {
            "binary_sha256": "b" * 64,
            "build_command": (
                "CC=gcc CXX=g++ cargo build --release "
                "-p fileconv-cli --no-default-features"
            ),
            "build_features": ["no-default-features"],
            "profile": "release",
        }
    first = render_markdown(data)
    second = render_markdown(json.loads(json.dumps(data)))

    assert first == second
    assert "# Phase A CPU OCR benchmark" in first
    assert "**STOP**" in first
    assert "1, 420, 839" in first
    assert "## Per-page quantitative metrics" in first
    assert "sample-1" in first
    assert "## Official sample runtime evidence" in first
    assert "| Markhand default | 420 | 2.500" in first
    assert "## Historical qualitative evidence" in first
    assert "`wikimedia-history`" in first
    assert "no trustworthy transcription" in first
    assert "## Reviewed multi-column reading order" in first
    assert "qualitative and limited" in first
    assert "MỤC TRÁI → MỤC PHẢI" in first
    assert (
        "| Markhand default | `wikimedia-history` | 1 | 2 | 2 | 1 | 0 | 0 |"
        in first
    )
    assert (
        "| Markhand default | `reviewed-multicolumn-v1` | 1 | 6 | 6 | 15 | 1 | 0 |"
        in first
    )
    reading_section = first.split(
        "## Reviewed multi-column reading order", maxsplit=1
    )[1].split("## Gate", maxsplit=1)[0]
    assert "| `wikimedia-history` | 2 |" not in reading_section
    assert "## Sample-size and representativeness limits" in first
    assert "9 real-scan pages" in first
    assert "not a population estimate" in first
    assert "## Cold initialization and resource semantics" in first
    assert "Warm median s/page" in first
    assert "sampled process-tree RSS" in first
    assert "## Candidate environment and build provenance" in first
    assert f"`{'b' * 64}`" in first
    assert "PYTHONNOUSERSITE=1" in first
    assert "fresh fileconv/Tesseract subprocess" in first
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
        ) -> RecognitionMeasurement:
            if page.source_id == "failed":
                raise RuntimeError("failure")
            return RecognitionMeasurement(
                text="reference",
                candidate_seconds=0.8,
                resource={
                    "method": "sampled_process_tree_rss",
                    "peak_rss_bytes": 100,
                    "sample_count": 4,
                    "sample_interval_seconds": 0.01,
                    "wall_seconds": 1.0,
                },
            )

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
    assert result["pages"][0]["elapsed_seconds"] == 1.0
    assert result["pages"][0]["candidate_seconds"] == 0.8
    assert result["pages"][0]["timing_scope"] == "warm_worker_request_wall"
    assert not result["pages"][0]["process_startup_included"]
    assert (
        result["pages"][0]["rss_measurement"]["method"]
        == "sampled_process_tree_rss"
    )


def test_process_tree_monitor_samples_transient_descendant_rss() -> None:
    script = """
import json
import subprocess
import sys
child = subprocess.Popen([
    sys.executable,
    "-c",
    "import time; payload = bytearray(48 * 1024 * 1024); time.sleep(0.2)",
])
child.wait()
print("unstructured native runtime diagnostic", flush=True)
print(json.dumps({"event": "done"}), flush=True)
"""
    process = subprocess.Popen(
        [sys.executable, "-c", script],
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
    )

    event, measurement = _read_event_with_process_tree_rss(
        process,
        timeout_seconds=5.0,
        max_rss_bytes=1024 * 1024 * 1024,
        sample_interval_seconds=0.005,
    )

    assert event == {"event": "done"}
    assert measurement["method"] == "sampled_process_tree_rss"
    assert measurement["sample_count"] > 2
    assert measurement["peak_rss_bytes"] > 48 * 1024 * 1024


def test_candidate_environment_is_allowlisted_and_report_safe(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SECRET_TOKEN", "must-not-propagate")
    monkeypatch.setenv("PYTHONPATH", "/secret/path")

    environment = sanitized_candidate_environment(cpu_threads=8)

    assert environment == {
        "LANG": "C.UTF-8",
        "LC_ALL": "C.UTF-8",
        "OMP_NUM_THREADS": "8",
        "OPENBLAS_NUM_THREADS": "8",
        "PATH": "/usr/local/bin:/usr/bin:/bin",
        "PYTHONNOUSERSITE": "1",
        "PYTHONPATH": "bench/ocr_cpu_service",
    }


def test_isolated_worker_separates_cold_start_from_warm_page(
    tmp_path: Path,
) -> None:
    worker_script = tmp_path / "worker.py"
    worker_script.write_text(
        """
import json
import sys
print(json.dumps({"event": "ready", "candidate_seconds": 0.02}), flush=True)
for line in sys.stdin:
    request = json.loads(line)
    if request["event"] == "shutdown":
        break
    print(json.dumps({
        "event": "result",
        "text": "reference",
        "candidate_seconds": 0.1,
    }), flush=True)
""",
        encoding="utf-8",
    )
    environment = sanitized_candidate_environment(cpu_threads=8)
    worker = IsolatedCandidateWorker(
        candidate_id="isolated",
        label="Isolated",
        command=[sys.executable, str(worker_script)],
        environment=environment,
        timeout_seconds=5.0,
        max_rss_bytes=1024 * 1024 * 1024,
    )
    page = BenchmarkPage(
        source_id="page",
        source_sha256="a" * 64,
        stratum="real-scan",
        page_number=1,
        path=tmp_path / "page.png",
        reference="reference",
    )
    try:
        measurement = worker.recognize(page)
    finally:
        worker.close()

    assert worker.metadata["environment_variable_names"] == sorted(environment)
    assert "environment" not in worker.metadata
    assert worker.metadata["cold_initialization"]["candidate_seconds"] == 0.02
    assert (
        worker.metadata["cold_initialization"]["timing_scope"]
        == "worker_process_invocation_to_ready"
    )
    assert worker.metadata["cold_initialization"]["process_startup_included"]
    assert (
        worker.metadata["cold_initialization"]["rss_measurement"]["method"]
        == "sampled_process_tree_rss"
    )
    assert measurement.text == "reference"
    assert measurement.candidate_seconds == 0.1
    assert measurement.resource["method"] == "sampled_process_tree_rss"


def test_fileconv_provenance_records_binary_hash_and_build_features(
    tmp_path: Path,
) -> None:
    binary = tmp_path / "fileconv"
    binary.write_bytes(b"release binary")

    provenance = _fileconv_provenance(binary)

    assert provenance == {
        "binary_sha256": (
            "9708beac508eb53b8ba9b8e7359a09237371a8a220a7c60da408e14c7a41cec4"
        ),
        "build_command": (
            "CC=gcc CXX=g++ cargo build --release "
            "-p fileconv-cli --no-default-features"
        ),
        "build_features": ["no-default-features"],
        "profile": "release",
    }
