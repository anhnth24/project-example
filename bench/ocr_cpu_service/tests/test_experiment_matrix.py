from __future__ import annotations

import json
import hashlib
import sys
from copy import deepcopy
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parents[1]))

from experiments.run_matrix import (  # noqa: E402
    EXPECTED_CANDIDATE_IDS,
    aggregate_records,
    build_candidate_specs,
    build_run_provenance,
    compare_repetitions,
    load_baseline_config,
    render_baseline_report,
    resolve_auto_tessdata,
    select_tuning_pages,
    validate_run_artifact,
)


SERVICE_ROOT = Path(__file__).parents[1]
BASELINE = SERVICE_ROOT / "experiments" / "baseline.json"


def _canonical_sha256(value: object) -> str:
    return hashlib.sha256(
        json.dumps(
            value, sort_keys=True, separators=(",", ":")
        ).encode()
    ).hexdigest()


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
        "split": "tuning",
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


def _artifact(repetition: int = 1) -> dict[str, object]:
    host = {
        "platform": "Linux-test",
        "architecture": "x86_64",
        "kernel_release": "test",
        "logical_cpus": 8,
        "physical_cpus": 4,
        "memory_bytes": 1024,
    }
    toolchain = {
        "python": {"implementation": "CPython", "version": "3.12.3"},
        "tesseract": ["tesseract 5.3.4"],
        "fileconv": {"package": "fileconv-cli", "version": "0.1.0"},
        "cargo": "cargo 1.88.0",
        "rustc": ["rustc 1.88.0"],
        "cc": {"tool": "gcc", "version": "gcc 13"},
        "cxx": {"tool": "g++", "version": "g++ 13"},
        "build": {
            "profile": "release",
            "features": ["no-default-features"],
            "environment_variable_names": ["CC", "CXX"],
        },
    }
    provenance = {
        "source_sha256": "1" * 64,
        "split_sha256": "2" * 64,
        "config_sha256": "3" * 64,
        "binary_sha256": "4" * 64,
        "tessdata_sha256": {
            "system-fast": {"vie": "5" * 64, "eng": "6" * 64},
            "auto": {"vie": "7" * 64, "eng": "8" * 64},
            "best": {"vie": "7" * 64, "eng": "8" * 64},
        },
        "host_sha256": _canonical_sha256(host),
        "toolchain_sha256": _canonical_sha256(toolchain),
    }
    records = [_record(f"p{index:02}") for index in range(44)]
    aggregate = aggregate_records(records)
    return {
        "schema_version": 1,
        "repetition": repetition,
        "split": "tuning",
        "page_count": 44,
        "provenance": provenance,
        "host": host,
        "toolchain": toolchain,
        "access": {
            "selected_tuning_pages": 44,
            "tuning_assets_checksummed": 44,
            "holdout_assets_resolved": 0,
            "holdout_assets_checksummed": 0,
            "holdout_assets_opened": 0,
            "holdout_ocr_executions": 0,
        },
        "candidates": [
            {
                "id": candidate_id,
                "label": candidate_id,
                "argv": ["/binary", "one", "{input}", "--lang", "vie+eng"],
                "environment_variable_names": (
                    ["LANG"]
                    if candidate_id == "markhand-auto"
                    else ["FILECONV_TESSDATA", "LANG"]
                ),
                "provenance": {
                    "binary_sha256": "4" * 64,
                    "toolchain_sha256": provenance["toolchain_sha256"],
                    "tessdata": {
                        "mode": {
                            "worker-system-fast": "system-fast",
                            "markhand-auto": "auto",
                            "tessdata-best": "best",
                        }[candidate_id],
                        "resolved_path": (
                            "/workspace/tessdata_best"
                            if candidate_id == "markhand-auto"
                            else None
                        ),
                        "sha256": provenance["tessdata_sha256"][
                            {
                                "worker-system-fast": "system-fast",
                                "markhand-auto": "auto",
                                "tessdata-best": "best",
                            }[candidate_id]
                        ],
                    },
                },
                "aggregate": aggregate,
                "strata": {"low-contrast": aggregate},
                "records": deepcopy(records),
                "diagnostics": deepcopy(records[:8]),
                "measurement": {
                    "cold_initialization": {
                        "candidate_seconds": 0.1,
                        "wall_seconds": 0.1,
                    },
                    "timing": "timing semantics",
                    "rss": "rss semantics",
                    "output": "output semantics",
                    "timeout": "timeout semantics",
                },
            }
            for candidate_id in EXPECTED_CANDIDATE_IDS
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
    assert tuple(candidate["id"] for candidate in config["candidates"]) == (
        EXPECTED_CANDIDATE_IDS
    )
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
    assert [candidate["tessdata_mode"] for candidate in config["candidates"]] == [
        "system-fast",
        "auto",
        "best",
    ]
    assert all("environment" not in candidate for candidate in config["candidates"])


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
        tessdata_paths={"system-fast": system, "auto": best, "best": best},
        host={"architecture": "x86_64", "logical_cpus": 8, "memory_bytes": 1024},
        toolchain={"python": "3.12.3", "tesseract": "tesseract 5.3.4"},
    )

    assert set(provenance) == {
        "source_sha256",
        "split_sha256",
        "config_sha256",
        "binary_sha256",
        "tessdata_sha256",
        "host_sha256",
        "toolchain_sha256",
    }
    assert set(provenance["tessdata_sha256"]) == {
        "system-fast",
        "auto",
        "best",
    }
    assert all(
        set(digests) == {"vie", "eng"}
        for digests in provenance["tessdata_sha256"].values()
    )


def test_candidate_specs_record_three_semantics_and_only_environment_names(
    tmp_path: Path,
) -> None:
    config = {
        "candidates": [
            {
                "id": candidate_id,
                "label": candidate_id,
                "argv": [
                    "{fileconv}",
                    "one",
                    "{input}",
                    "--lang",
                    "vie+eng",
                ],
                "tessdata_mode": mode,
            }
            for candidate_id, mode in zip(
                EXPECTED_CANDIDATE_IDS,
                ("system-fast", "auto", "best"),
                strict=True,
            )
        ]
    }

    specs, public = build_candidate_specs(
        config,
        fileconv=tmp_path / "fileconv",
        system_tessdata=tmp_path / "system",
        best_tessdata=tmp_path / "best",
        auto_tessdata=tmp_path / "best",
        cpu_threads=1,
        binary_sha256="a" * 64,
        tessdata_sha256={
            "system-fast": {"vie": "b" * 64, "eng": "c" * 64},
            "auto": {"vie": "d" * 64, "eng": "e" * 64},
            "best": {"vie": "d" * 64, "eng": "e" * 64},
        },
        toolchain_sha256="f" * 64,
    )

    assert all(spec.argv == (
        str(tmp_path / "fileconv"),
        "one",
        "{input}",
        "--lang",
        "vie+eng",
    ) for spec in specs)
    explicit_names = sorted(
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
    auto_names = [name for name in explicit_names if name != "FILECONV_TESSDATA"]
    assert public[0]["environment_variable_names"] == explicit_names
    assert public[1]["environment_variable_names"] == auto_names
    assert public[2]["environment_variable_names"] == explicit_names
    assert public[1]["provenance"]["tessdata"]["resolved_path"] == str(
        tmp_path / "best"
    )
    serialized = json.dumps(public)
    assert '"environment"' not in serialized
    assert "CC=gcc" not in serialized
    assert str(tmp_path / "system") not in serialized


def test_auto_tessdata_resolution_matches_core_search_order(tmp_path: Path) -> None:
    cwd = tmp_path / "checkout" / "nested"
    executable = tmp_path / "checkout" / "target" / "release" / "fileconv"
    manifest = tmp_path / "checkout" / "crates" / "core"
    cwd.mkdir(parents=True)
    executable.parent.mkdir(parents=True)
    manifest.mkdir(parents=True)
    discovered = tmp_path / "checkout" / "tessdata_best"
    discovered.mkdir()
    (discovered / "vie.traineddata").write_bytes(b"vie")
    (discovered / "eng.traineddata").write_bytes(b"eng")

    assert resolve_auto_tessdata(
        cwd=cwd,
        executable=executable,
        manifest_dir=manifest,
    ) == discovered


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


@pytest.mark.parametrize(
    ("mutation", "message"),
    [
        (lambda run: run.update(repetition=3), "repetition"),
        (lambda run: run.update(page_count=43), "44"),
        (
            lambda run: run["candidates"][0].update(id="renamed"),
            "candidate IDs",
        ),
        (lambda run: run["candidates"].pop(), "candidate IDs"),
        (
            lambda run: run["candidates"][0]["records"].pop(),
            "44 unique",
        ),
        (
            lambda run: run["candidates"][0]["records"].__setitem__(
                -1, deepcopy(run["candidates"][0]["records"][0])
            ),
            "duplicate",
        ),
        (
            lambda run: run["access"].update(holdout_assets_opened=1),
            "holdout",
        ),
        (
            lambda run: run["candidates"][0]["records"][0].update(
                split="holdout"
            ),
            "holdout",
        ),
        (
            lambda run: run["candidates"][0]["aggregate"].update(successes=43),
            "cardinality",
        ),
        (
            lambda run: run["candidates"][0]["records"][0].update(
                success=False, error_kind=None
            ),
            "cardinality",
        ),
    ],
)
def test_artifact_validation_rejects_cardinality_identity_or_holdout_drift(
    mutation, message: str
) -> None:
    artifact = _artifact()
    mutation(artifact)

    with pytest.raises(ValueError, match=message):
        validate_run_artifact(artifact)


def test_artifact_validation_rejects_malformed_failure_record() -> None:
    artifact = _artifact()
    candidate = artifact["candidates"][0]
    record = candidate["records"][0]
    record["success"] = False
    record["error_kind"] = None
    for field in (
        "character_edits",
        "reference_characters",
        "word_edits",
        "reference_words",
    ):
        record.pop(field)
    candidate["aggregate"] = aggregate_records(candidate["records"])
    candidate["strata"]["low-contrast"] = aggregate_records(candidate["records"])

    with pytest.raises(ValueError, match="failure record"):
        validate_run_artifact(artifact)


@pytest.mark.parametrize(
    ("section", "field", "value", "message"),
    [
        ("host", "kernel_release", "changed", "host checksum"),
        ("toolchain", "python", "3.13.0", "toolchain checksum"),
        ("toolchain", "tesseract", "tesseract 6", "toolchain checksum"),
        (
            "toolchain",
            "fileconv",
            {"package": "fileconv-cli", "version": "9.9.9"},
            "toolchain checksum",
        ),
    ],
)
def test_unhashed_host_or_version_drift_is_rejected(
    section: str, field: str, value: object, message: str
) -> None:
    artifact = _artifact()
    artifact[section][field] = value

    with pytest.raises(ValueError, match=message):
        validate_run_artifact(artifact)


def test_repetition_comparison_rejects_count_or_provenance_drift_but_not_timing() -> None:
    first = _artifact()
    second = _artifact(repetition=2)
    second["candidates"][0]["records"][0]["elapsed_seconds"] = 0.9
    second["candidates"][0]["records"][0]["peak_rss_bytes"] = 200
    second["candidates"][0]["diagnostics"] = deepcopy(
        second["candidates"][0]["records"][:8]
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
    assert comparison["timing_variance"]["worker-system-fast"]["max_seconds"] == 0.9

    drifted = deepcopy(second)
    drifted["candidates"][0]["records"][0]["character_edits"] = 3
    drifted["candidates"][0]["diagnostics"] = deepcopy(
        drifted["candidates"][0]["records"][:8]
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

    with pytest.raises(ValueError, match="repetitions 1 and 2"):
        compare_repetitions(first, deepcopy(first))


def test_report_is_raw_recomputable_and_states_holdout_blocker() -> None:
    first = _artifact()
    second = _artifact(repetition=2)

    markdown = render_baseline_report(first, second)

    assert "| p00 | `low-contrast` | 2 | 20 | 1 | 4 |" in markdown
    assert "88 / 880 = 0.100000" in markdown
    assert "Holdout assets were not read or executed" in markdown
    assert "production remains blocked" in markdown
    assert "recognized_text" not in markdown
    assert "reference" not in markdown.lower()

