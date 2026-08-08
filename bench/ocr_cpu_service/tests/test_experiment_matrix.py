from __future__ import annotations

import json
import sys
from copy import deepcopy
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parents[1]))

from experiments.run_matrix import (  # noqa: E402
    aggregate_records,
    build_candidate_specs,
    build_run_provenance,
    compare_repetitions,
    load_baseline_config,
    render_baseline_report,
    select_tuning_pages,
    validate_run_artifact,
)


SERVICE_ROOT = Path(__file__).parents[1]
BASELINE = SERVICE_ROOT / "experiments" / "baseline.json"


def _record(
    page_id: str,
    *,
    character_edits: int = 2,
    reference_characters: int = 20,
    word_edits: int = 1,
    reference_words: int = 4,
    strata: tuple[str, ...] = ("low-contrast",),
    elapsed_seconds: float = 0.5,
    peak_rss_bytes: int = 100,
) -> dict[str, object]:
    return {
        "page_id": page_id,
        "source_id": f"source-{page_id}",
        "page_number": 1,
        "difficulty_strata": list(strata),
        "success": True,
        "error_kind": None,
        "character_edits": character_edits,
        "reference_characters": reference_characters,
        "word_edits": word_edits,
        "reference_words": reference_words,
        "elapsed_seconds": elapsed_seconds,
        "peak_rss_bytes": peak_rss_bytes,
        "resource_limit_violation": False,
    }


def _artifact() -> dict[str, object]:
    provenance = {
        name: character * 64
        for name, character in (
            ("source_sha256", "1"),
            ("split_sha256", "2"),
            ("config_sha256", "3"),
            ("binary_sha256", "4"),
            ("tessdata_sha256", "5"),
            ("host_sha256", "6"),
        )
    }
    records = [_record("p1")]
    aggregate = aggregate_records(records)
    return {
        "schema_version": 1,
        "repetition": 1,
        "split": "tuning",
        "page_count": 1,
        "provenance": provenance,
        "candidates": [
            {
                "id": "markhand-default",
                "argv": ["/binary", "one", "{input}", "--lang", "vie+eng"],
                "environment_variable_names": ["FILECONV_TESSDATA", "LANG"],
                "provenance": {
                    "binary_sha256": "4" * 64,
                    "tessdata_sha256": {
                        "vie": "7" * 64,
                        "eng": "8" * 64,
                    },
                },
                "aggregate": aggregate,
                "strata": {"low-contrast": aggregate},
                "records": records,
                "diagnostics": records,
            }
        ],
    }


def test_checked_in_config_locks_tuning_and_exact_candidate_interfaces() -> None:
    config = load_baseline_config(BASELINE)

    assert config["split"] == "tuning"
    assert config["expected_pages"] == 44
    assert config["repetitions"] == 2
    assert config["limits"] == {
        "cpu_threads": 1,
        "diagnostic_pages_per_candidate": 8,
        "max_output_bytes_per_stream": 1048576,
        "max_rss_bytes": 4294967296,
        "timeout_seconds_per_page": 180,
    }
    assert [candidate["id"] for candidate in config["candidates"]] == [
        "markhand-default",
        "markhand-tessdata-best",
    ]
    assert all(
        candidate["argv"] == [
            "{fileconv}",
            "one",
            "{input}",
            "--lang",
            "vie+eng",
        ]
        for candidate in config["candidates"]
    )
    assert config["candidates"][0]["environment"] == {
        "FILECONV_TESSDATA": "{system_tessdata}"
    }
    assert config["candidates"][1]["environment"] == {
        "FILECONV_TESSDATA": "{best_tessdata}"
    }


def test_provenance_requires_source_split_config_binary_tessdata_and_host(
    tmp_path: Path,
) -> None:
    source = tmp_path / "sources.json"
    annotations = tmp_path / "annotations.jsonl"
    config = tmp_path / "baseline.json"
    binary = tmp_path / "fileconv"
    system = tmp_path / "system"
    best = tmp_path / "best"
    system.mkdir()
    best.mkdir()
    for path, content in (
        (source, b"source"),
        (annotations, b"annotations"),
        (config, b"config"),
        (binary, b"binary"),
        (system / "vie.traineddata", b"system-vie"),
        (system / "eng.traineddata", b"system-eng"),
        (best / "vie.traineddata", b"best-vie"),
        (best / "eng.traineddata", b"best-eng"),
    ):
        path.write_bytes(content)

    provenance = build_run_provenance(
        source_manifest=source,
        split_payload=b"canonical tuning split",
        config_path=config,
        fileconv=binary,
        tessdata_paths={"system": system, "best": best},
        host={"architecture": "x86_64", "logical_cpus": 8, "memory_bytes": 1024},
    )

    assert set(provenance) == {
        "source_sha256",
        "split_sha256",
        "config_sha256",
        "binary_sha256",
        "tessdata_sha256",
        "host_sha256",
    }
    assert set(provenance["tessdata_sha256"]) == {"system", "best"}
    assert all(
        set(digests) == {"vie", "eng"}
        for digests in provenance["tessdata_sha256"].values()
    )


def test_candidate_specs_record_exact_argv_and_only_environment_names(
    tmp_path: Path,
) -> None:
    config = {
        "candidates": [
            {
                "id": "markhand-default",
                "label": "Markhand default",
                "argv": [
                    "{fileconv}",
                    "one",
                    "{input}",
                    "--lang",
                    "vie+eng",
                ],
                "environment": {"FILECONV_TESSDATA": "{system_tessdata}"},
            }
        ]
    }

    specs, public = build_candidate_specs(
        config,
        fileconv=tmp_path / "fileconv",
        system_tessdata=tmp_path / "system",
        best_tessdata=tmp_path / "best",
        cpu_threads=1,
        binary_sha256="a" * 64,
        tessdata_sha256={"system": {"vie": "b" * 64, "eng": "c" * 64}},
    )

    assert specs[0].argv == (
        str(tmp_path / "fileconv"),
        "one",
        "{input}",
        "--lang",
        "vie+eng",
    )
    assert public[0]["argv"] == list(specs[0].argv)
    assert public[0]["environment_variable_names"] == sorted(
        {
            "FILECONV_TESSDATA",
            "LANG",
            "LC_ALL",
            "OMP_NUM_THREADS",
            "OPENBLAS_NUM_THREADS",
            "PATH",
            "PYTHONNOUSERSITE",
            "PYTHONPATH",
        }
    )
    assert "environment" not in public[0]
    assert str(tmp_path / "system") not in json.dumps(public[0])


def test_tuning_selection_never_resolves_holdout_asset(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    tuning = {
        "page_id": "tuning-page",
        "split": "tuning",
        "asset_path": "tuning.png",
        "image_sha256": "a" * 64,
        "source_id": "source",
        "source_sha256": "b" * 64,
        "page_number": 1,
        "transcription": "reference",
        "difficulty_strata": ["clean-official"],
    }
    holdout = {
        **tuning,
        "page_id": "holdout-page",
        "split": "holdout",
        "asset_path": "holdout.png",
        "image_sha256": "c" * 64,
    }
    touched: list[Path] = []

    def checksum(path: Path) -> str:
        touched.append(path)
        return "a" * 64

    pages, _ = select_tuning_pages(
        [tuning, holdout],
        assets_dir=tmp_path,
        expected_pages=1,
        checksum=checksum,
    )

    assert [page["page_id"] for page in pages] == ["tuning-page"]
    assert touched == [tmp_path / "tuning.png"]


def test_aggregate_preserves_raw_additive_counts_and_overlapping_strata() -> None:
    records = [
        _record("p1", strata=("low-contrast", "small-text")),
        _record(
            "p2",
            character_edits=3,
            reference_characters=30,
            word_edits=2,
            reference_words=6,
            strata=("small-text",),
        ),
    ]

    aggregate = aggregate_records(records)
    small_text = aggregate_records(
        [record for record in records if "small-text" in record["difficulty_strata"]]
    )

    assert aggregate["raw_counts"] == {
        "character_edits": 5,
        "reference_characters": 50,
        "word_edits": 3,
        "reference_words": 10,
    }
    assert aggregate["cer"] == 0.1
    assert aggregate["wer"] == 0.3
    assert small_text["raw_counts"] == aggregate["raw_counts"]


@pytest.mark.parametrize(
    ("mutation", "message"),
    [
        (lambda run: run.update(split="holdout"), "tuning"),
        (
            lambda run: run["candidates"][0]["records"][0].update(
                recognized_text="secret"
            ),
            "recognized text",
        ),
        (
            lambda run: run["provenance"].pop("host_sha256"),
            "provenance",
        ),
        (
            lambda run: run["provenance"].update(binary_sha256="not-a-checksum"),
            "checksum",
        ),
        (
            lambda run: run["candidates"][0].update(
                environment={"SECRET_TOKEN": "secret"}
            ),
            "environment values",
        ),
    ],
)
def test_run_artifact_fails_closed_on_leakage_or_missing_provenance(
    mutation, message: str
) -> None:
    artifact = _artifact()
    mutation(artifact)

    with pytest.raises(ValueError, match=message):
        validate_run_artifact(artifact)


def test_repetition_comparison_rejects_count_or_provenance_drift_but_not_timing() -> None:
    first = _artifact()
    second = deepcopy(first)
    second["repetition"] = 2
    second["candidates"][0]["records"][0]["elapsed_seconds"] = 0.9
    second["candidates"][0]["records"][0]["peak_rss_bytes"] = 200
    second["candidates"][0]["diagnostics"] = deepcopy(
        second["candidates"][0]["records"]
    )
    second["candidates"][0]["aggregate"] = aggregate_records(
        second["candidates"][0]["records"]
    )
    second["candidates"][0]["strata"]["low-contrast"] = aggregate_records(
        second["candidates"][0]["records"]
    )

    comparison = compare_repetitions(first, second)

    assert comparison["deterministic_counts"] is True
    assert comparison["timing_compared_for_determinism"] is False
    assert comparison["timing_variance"]["markhand-default"]["max_seconds"] == 0.9

    drifted = deepcopy(second)
    drifted["candidates"][0]["records"][0]["character_edits"] = 3
    drifted["candidates"][0]["diagnostics"] = deepcopy(
        drifted["candidates"][0]["records"]
    )
    drifted["candidates"][0]["aggregate"] = aggregate_records(
        drifted["candidates"][0]["records"]
    )
    drifted["candidates"][0]["strata"]["low-contrast"] = aggregate_records(
        drifted["candidates"][0]["records"]
    )
    with pytest.raises(ValueError, match="OCR count"):
        compare_repetitions(first, drifted)

    changed_provenance = deepcopy(second)
    changed_provenance["provenance"]["binary_sha256"] = "9" * 64
    with pytest.raises(ValueError, match="provenance"):
        compare_repetitions(first, changed_provenance)


def test_report_is_raw_recomputable_and_states_holdout_blocker() -> None:
    first = _artifact()
    second = deepcopy(first)
    second["repetition"] = 2

    markdown = render_baseline_report(first, second)

    assert "| p1 | 2 | 20 | 1 | 4 |" in markdown
    assert "2 / 20 = 0.100000" in markdown
    assert "Holdout assets were not read or executed" in markdown
    assert "production remains blocked" in markdown
    assert "recognized_text" not in markdown
    assert "reference" not in markdown.lower()

