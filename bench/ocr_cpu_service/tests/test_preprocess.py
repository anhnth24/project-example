from __future__ import annotations

import hashlib
import json
import os
import signal
import subprocess
import sys
import time
from copy import deepcopy
from pathlib import Path

import pytest
from PIL import Image, ImageDraw

sys.path.insert(0, str(Path(__file__).parents[1]))

from experiments.preprocess import (  # noqa: E402
    MAX_DESKEW_DEGREES,
    MAX_DIMENSION,
    MAX_EVENT_BYTES,
    MAX_INPUT_BYTES,
    MAX_PIXELS,
    _build_specs,
    _event_summary,
    _rotate_expand_white,
    aggregate_matrix_records,
    canonical_config_checksum,
    load_experiment_configs,
    render_matrix_report,
    transform_image,
    validate_matrix_artifact,
)


SERVICE_ROOT = Path(__file__).parents[1]
CONFIGS = SERVICE_ROOT / "experiments" / "configs.json"
PREPROCESS = SERVICE_ROOT / "experiments" / "preprocess.py"
EXPECTED_IDS = (
    "control",
    "direct-tesseract-transfer-control",
    "dpi-hint-400",
    "deskew-auto-bounded",
    "grayscale-normalization",
    "threshold-otsu",
    "threshold-adaptive-pillow",
    "median-denoise",
    "background-watermark-suppression",
    "psm-3",
    "psm-4",
    "psm-6",
    "psm-11",
    "system-fast-tessdata",
)


def _write_sample(path: Path) -> None:
    image = Image.new("L", (180, 100), 255)
    draw = ImageDraw.Draw(image)
    draw.rectangle((12, 20, 165, 72), outline=0, width=2)
    draw.line((18, 48, 156, 48), fill=70, width=2)
    draw.point((90, 30), fill=130)
    image.save(path, format="PNG", optimize=False, compress_level=9)


def _record(
    page_id: str,
    *,
    document_type: str = "modern-government",
    strata: tuple[str, ...] = ("low-contrast",),
    edits: int = 2,
) -> dict[str, object]:
    transform_sha256s = ["a" * 64]
    return {
        "page_id": page_id,
        "split": "tuning",
        "source_id": f"source-{page_id}",
        "source_sha256": "f" * 64,
        "page_number": 1,
        "document_type": document_type,
        "difficulty_strata": list(strata),
        "success": True,
        "error_kind": None,
        "character_edits": edits,
        "reference_characters": 20,
        "word_edits": 1,
        "reference_words": 4,
        "elapsed_seconds": 0.5,
        "peak_rss_bytes": 100,
        "resource_limit_violation": False,
        "input_sha256s": ["d" * 64],
        "transform_sha256s": transform_sha256s,
        "transform_sha256": hashlib.sha256(
            json.dumps(
                transform_sha256s,
                ensure_ascii=False,
                sort_keys=True,
                separators=(",", ":"),
            ).encode()
        ).hexdigest(),
        "transform_attempts": 1,
    }


def _artifact() -> dict[str, object]:
    configs = load_experiment_configs(CONFIGS)
    candidates = []
    for config in configs["configs"]:
        records = [
            _record(
                f"p{index:02}",
                document_type=(
                    "modern-government" if index < 9 else "historical-old-print"
                ),
            )
            for index in range(44)
        ]
        records[0].update(
            character_edits=6707 - 43 * 2,
            reference_characters=54897 - 43 * 20,
            word_edits=3383 - 43,
            reference_words=11959 - 43 * 4,
        )
        if config["id"] == "control":
            for record in records:
                for key in (
                    "input_sha256s",
                    "transform_sha256s",
                    "transform_sha256",
                    "transform_attempts",
                ):
                    record.pop(key)
        candidates.append(
            {
                "id": config["id"],
                "label": config["label"],
                "changed_factor": config["changed_factor"],
                "factor": config["factor"],
                "configuration_sha256": config["configuration_sha256"],
                "execution_class": (
                    "rust-control"
                    if config["id"] == "control"
                    else "direct-tesseract-experiment"
                ),
                "environment_variable_names": (
                    ["FILECONV_TESSDATA", "LANG"]
                    if config["id"] == "control"
                    else [
                        "BENCH_OCR_EXPERIMENT_CONFIG_ID",
                        "BENCH_OCR_EXPERIMENT_EVENTS",
                        "BENCH_OCR_REAL_TESSERACT",
                        "FILECONV_TESSERACT",
                        "FILECONV_TESSDATA",
                        "LANG",
                    ]
                ),
                "records": records,
                "aggregate": aggregate_matrix_records(records),
                "strata": {},
                "document_types": {},
                "diagnostics": records[:8],
            }
        )
    for candidate in candidates:
        records = candidate["records"]
        candidate["strata"] = {
            "low-contrast": aggregate_matrix_records(records),
        }
        candidate["document_types"] = {
            kind: aggregate_matrix_records(
                [row for row in records if row["document_type"] == kind]
            )
            for kind in ("historical-old-print", "modern-government")
        }
    return {
        "schema_version": 1,
        "split": "tuning",
        "page_count": 44,
        "provenance": {
            "source_sha256": "1" * 64,
            "split_sha256": "2" * 64,
            "configs_sha256": "3" * 64,
            "binary_sha256": "4" * 64,
            "shim_sha256": "5" * 64,
            "python_binary_sha256": "e" * 64,
            "tesseract_binary_sha256": "d" * 64,
            "baseline_config_sha256": "6" * 64,
            "host_sha256": "7" * 64,
            "toolchain_sha256": "8" * 64,
            "tessdata_sha256": {
                "system-fast": {"vie": "9" * 64, "eng": "a" * 64},
                "best": {"vie": "b" * 64, "eng": "c" * 64},
            },
        },
        "access": {
            "selected_tuning_pages": 44,
            "tuning_assets_checksummed": 44,
            "holdout_assets_resolved": 0,
            "holdout_assets_checksummed": 0,
            "holdout_assets_opened": 0,
            "holdout_ocr_executions": 0,
        },
        "baseline_calibration": {
            "expected_character_edits": 6707,
            "expected_reference_characters": 54897,
            "expected_word_edits": 3383,
            "expected_reference_words": 11959,
            "rust_control_matches_task2_best": True,
            "direct_transfer_character_edit_gap": 0,
            "direct_transfer_word_edit_gap": 0,
            "direct_transfer_counts_match": True,
        },
        "candidates": candidates,
    }


def test_checked_in_configs_are_unique_one_factor_changes_with_exact_checksums() -> None:
    payload = load_experiment_configs(CONFIGS)

    assert payload["split"] == "tuning"
    assert payload["expected_pages"] == 44
    assert tuple(item["id"] for item in payload["configs"]) == EXPECTED_IDS
    assert payload["configs"][0]["changed_factor"] is None
    assert payload["configs"][0]["factor"] == {}
    assert sum(item["changed_factor"] is None for item in payload["configs"]) == 1
    for item in payload["configs"]:
        assert item["configuration_sha256"] == canonical_config_checksum(item)
        assert len(item["factor"]) == (0 if item["id"] == "control" else 1)
        assert tuple(item["factor"]) == (
            () if item["id"] == "control" else (item["changed_factor"],)
        )


@pytest.mark.parametrize(
    ("mutation", "message"),
    [
        (lambda value: value["configs"].pop(0), "control"),
        (
            lambda value: value["configs"].append(deepcopy(value["configs"][0])),
            "duplicate",
        ),
        (
            lambda value: value["configs"][1]["factor"].update(psm=4),
            "one factor",
        ),
        (
            lambda value: value["configs"][1].update(configuration_sha256="0" * 64),
            "checksum",
        ),
    ],
)
def test_config_loader_fails_closed_on_missing_duplicate_multifactor_or_drift(
    tmp_path: Path, mutation, message: str
) -> None:
    value = json.loads(CONFIGS.read_text())
    mutation(value)
    path = tmp_path / "configs.json"
    path.write_text(json.dumps(value))

    with pytest.raises(ValueError, match=message):
        load_experiment_configs(path)


@pytest.mark.parametrize(
    "factor",
    [
        {"deskew": {
            "method": "projection-auto",
            "max_degrees": 3.0,
            "step_degrees": 0.25,
            "expand": True,
            "fill": "white",
        }},
        {"grayscale_normalization": {
            "method": "autocontrast",
            "cutoff_percent": 0,
        }},
        {"threshold": {"method": "otsu"}},
        {"threshold": {
            "method": "adaptive-pillow",
            "window": 31,
            "offset": 10,
        }},
        {"denoise": {"method": "median", "size": 3}},
        {"background_suppression": {
            "method": "light-tone-compress",
            "preserve_below": 180,
            "gain": 2,
        }},
    ],
)
def test_transforms_are_deterministic_non_cropping_and_cleanup_temp_files(
    tmp_path: Path, factor: dict[str, object],
) -> None:
    source = tmp_path / "source.png"
    _write_sample(source)
    with transform_image(source, factor, work_dir=tmp_path / "work") as first:
        first_checksum = first.sha256
        assert first.path.is_file()
        assert first.width >= 180
        assert first.height >= 100
    assert not first.path.exists()

    with transform_image(source, factor, work_dir=tmp_path / "work") as second:
        assert second.sha256 == first_checksum
        assert hashlib.sha256(second.path.read_bytes()).hexdigest() == first_checksum
    assert list((tmp_path / "work").iterdir()) == []


def test_image_bounds_fail_closed_before_transform(tmp_path: Path) -> None:
    source = tmp_path / "too-wide.png"
    Image.new("1", (MAX_DIMENSION + 1, 1), 1).save(source)
    with pytest.raises(ValueError, match="dimension"):
        with transform_image(source, {}, work_dir=tmp_path / "work"):
            pass

    assert MAX_PIXELS < MAX_DIMENSION * MAX_DIMENSION


def test_input_byte_bound_fails_before_image_decode(tmp_path: Path) -> None:
    source = tmp_path / "oversized.bin"
    with source.open("wb") as stream:
        stream.truncate(MAX_INPUT_BYTES + 1)

    with pytest.raises(ValueError, match="input byte"):
        with transform_image(source, {}, work_dir=tmp_path / "work"):
            pass


def test_event_reader_rejects_unbounded_or_excessive_attempt_logs(
    tmp_path: Path,
) -> None:
    events = tmp_path / "events.jsonl"
    with events.open("wb") as stream:
        stream.truncate(MAX_EVENT_BYTES + 1)

    with pytest.raises(ValueError, match="event"):
        _event_summary(events)


def test_deskew_rotation_is_bounded_expands_and_uses_white_padding() -> None:
    image = Image.new("L", (100, 50), 255)
    ImageDraw.Draw(image).rectangle((0, 0, 99, 49), outline=0)

    rotated, applied = _rotate_expand_white(image, MAX_DESKEW_DEGREES + 4)

    assert applied == MAX_DESKEW_DEGREES
    assert rotated.width > image.width
    assert rotated.height > image.height
    assert rotated.getpixel((0, 0)) == 255
    assert rotated.getbbox() == (0, 0, rotated.width, rotated.height)


def _write_fake_tesseract(path: Path, *, exit_code: int = 0, sleep: bool = False) -> None:
    body = [
        "#!/usr/bin/env python3",
        "import json, os, signal, sys, time",
        "capture = os.environ.get('FAKE_CAPTURE')",
        "if capture:",
        "    open(capture, 'w').write(json.dumps(sys.argv[1:]))",
    ]
    if sleep:
        body.extend(
            [
                "signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))",
                "time.sleep(30)",
            ]
        )
    else:
        body.extend(["print('recognized')", f"sys.exit({exit_code})"])
    path.write_text("\n".join(body) + "\n")
    path.chmod(0o755)


def _shim_environment(
    tmp_path: Path, real: Path, events: Path, config_id: str
) -> dict[str, str]:
    return {
        **os.environ,
        "BENCH_OCR_REAL_TESSERACT": str(real),
        "BENCH_OCR_EXPERIMENT_CONFIG_ID": config_id,
        "BENCH_OCR_EXPERIMENT_CONFIGS": str(CONFIGS),
        "BENCH_OCR_EXPERIMENT_EVENTS": str(events),
        "BENCH_OCR_EXPERIMENT_WORK_DIR": str(tmp_path / "work"),
        "PYTHONPATH": str(SERVICE_ROOT),
    }


def test_shim_preserves_direct_argv_and_records_preprocessed_transform_checksum(
    tmp_path: Path,
) -> None:
    source = tmp_path / "preprocessed.png"
    _write_sample(source)
    real = tmp_path / "fake-tesseract"
    capture = tmp_path / "argv.json"
    events = tmp_path / "events.jsonl"
    _write_fake_tesseract(real)
    env = _shim_environment(tmp_path, real, events, "dpi-hint-400")
    env["FAKE_CAPTURE"] = str(capture)
    hostile = "$(touch should-not-exist)"

    result = subprocess.run(
        [
            sys.executable,
            str(PREPROCESS),
            str(source),
            "stdout",
            "-l",
            "vie+eng",
            "--psm",
            "3",
            "--dpi",
            "300",
            hostile,
        ],
        env=env,
        capture_output=True,
        text=True,
        check=True,
    )

    argv = json.loads(capture.read_text())
    event = json.loads(events.read_text())
    assert result.stdout == "recognized\n"
    assert hostile in argv
    assert not (Path.cwd() / "should-not-exist").exists()
    assert argv[0] == str(source)
    assert argv[argv.index("--dpi") + 1] == "400"
    assert event["config_id"] == "dpi-hint-400"
    assert event["input_sha256"] == hashlib.sha256(source.read_bytes()).hexdigest()
    assert event["transform_sha256"] == event["input_sha256"]
    assert list((tmp_path / "work").iterdir()) == []


def test_matrix_candidate_environment_runs_shim_with_pinned_python(
    tmp_path: Path,
) -> None:
    real = tmp_path / "fake-tesseract"
    _write_fake_tesseract(real)
    specs, _, _ = _build_specs(
        load_experiment_configs(CONFIGS),
        configs_path=CONFIGS,
        fileconv=tmp_path / "fileconv",
        shim=PREPROCESS,
        real_tesseract=real,
        system_tessdata=tmp_path / "tessdata",
        best_tessdata=tmp_path / "tessdata-best",
        work_dir=tmp_path / "work",
    )

    control, transfer = specs[:2]
    assert "FILECONV_TESSDATA" in control.environment
    assert "FILECONV_TESSERACT" not in control.environment
    assert "BENCH_OCR_EXPERIMENT_CONFIG_ID" not in control.environment
    assert "FILECONV_TESSERACT" in transfer.environment
    assert "BENCH_OCR_EXPERIMENT_CONFIG_ID" in transfer.environment

    result = subprocess.run(
        [str(PREPROCESS), "--version"],
        env=transfer.environment,
        capture_output=True,
        text=True,
    )

    assert result.returncode == 0, result.stderr
    assert Path(sys.executable).parent == Path(
        transfer.environment["PATH"].split(os.pathsep)[0]
    )


def test_matrix_candidate_binds_the_exact_supplied_config_path(
    tmp_path: Path,
) -> None:
    custom = tmp_path / "custom-configs.json"
    custom.write_bytes(CONFIGS.read_bytes())
    specs, _, _ = _build_specs(
        load_experiment_configs(custom),
        configs_path=custom,
        fileconv=tmp_path / "fileconv",
        shim=PREPROCESS,
        real_tesseract=tmp_path / "tesseract",
        system_tessdata=tmp_path / "tessdata",
        best_tessdata=tmp_path / "tessdata-best",
        work_dir=tmp_path / "work",
    )

    assert specs[1].environment["BENCH_OCR_EXPERIMENT_CONFIGS"] == str(
        custom.resolve()
    )


def test_control_is_real_rust_pipeline_and_direct_variants_have_transfer_control(
    tmp_path: Path,
) -> None:
    configs = load_experiment_configs(CONFIGS)
    specs, public, events = _build_specs(
        configs,
        configs_path=CONFIGS,
        fileconv=tmp_path / "fileconv",
        shim=PREPROCESS,
        real_tesseract=tmp_path / "tesseract",
        system_tessdata=tmp_path / "system-tessdata",
        best_tessdata=tmp_path / "best-tessdata",
        work_dir=tmp_path / "work",
    )

    assert specs[0].id == "control"
    assert specs[0].environment["FILECONV_TESSDATA"] == str(
        tmp_path / "best-tessdata"
    )
    assert not set(specs[0].environment) & {
        "FILECONV_TESSERACT",
        "BENCH_OCR_EXPERIMENT_CONFIG_ID",
        "BENCH_OCR_EXPERIMENT_CONFIGS",
        "BENCH_OCR_EXPERIMENT_EVENTS",
        "BENCH_OCR_EXPERIMENT_WORK_DIR",
        "BENCH_OCR_REAL_TESSERACT",
    }
    assert public[0]["execution_class"] == "rust-control"
    assert events[0] is None

    assert specs[1].id == "direct-tesseract-transfer-control"
    assert public[1]["execution_class"] == "direct-tesseract-experiment"
    assert events[1] is not None
    assert all(
        item["execution_class"] == "direct-tesseract-experiment"
        for item in public[1:]
    )


def test_shim_rejects_non_stdout_output_target(tmp_path: Path) -> None:
    source = tmp_path / "preprocessed.png"
    _write_sample(source)
    real = tmp_path / "fake-tesseract"
    capture = tmp_path / "argv.json"
    events = tmp_path / "events.jsonl"
    _write_fake_tesseract(real)
    env = _shim_environment(tmp_path, real, events, "control")
    env["FAKE_CAPTURE"] = str(capture)

    result = subprocess.run(
        [sys.executable, str(PREPROCESS), str(source), str(tmp_path / "output")],
        env=env,
        capture_output=True,
        text=True,
    )

    assert result.returncode != 0
    assert not capture.exists()
    assert not (tmp_path / "output.txt").exists()


@pytest.mark.parametrize("exit_code", [0, 7])
def test_shim_cleans_transformed_temp_on_success_and_failure(
    tmp_path: Path, exit_code: int
) -> None:
    source = tmp_path / "preprocessed.png"
    _write_sample(source)
    real = tmp_path / "fake-tesseract"
    events = tmp_path / "events.jsonl"
    _write_fake_tesseract(real, exit_code=exit_code)

    result = subprocess.run(
        [sys.executable, str(PREPROCESS), str(source), "stdout"],
        env=_shim_environment(tmp_path, real, events, "threshold-otsu"),
    )

    assert result.returncode == exit_code
    assert list((tmp_path / "work").iterdir()) == []


def test_shim_signal_cleanup_removes_temp_and_terminates_child(tmp_path: Path) -> None:
    source = tmp_path / "preprocessed.png"
    _write_sample(source)
    real = tmp_path / "fake-tesseract"
    events = tmp_path / "events.jsonl"
    _write_fake_tesseract(real, sleep=True)
    work = tmp_path / "work"
    process = subprocess.Popen(
        [sys.executable, str(PREPROCESS), str(source), "stdout"],
        env=_shim_environment(tmp_path, real, events, "threshold-otsu"),
    )
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if work.exists() and list(work.iterdir()):
            break
        time.sleep(0.02)
    else:
        process.kill()
        pytest.fail("shim did not create its transformed temporary image")

    process.send_signal(signal.SIGTERM)
    process.wait(timeout=5)

    assert list(work.iterdir()) == []


def test_matrix_artifact_requires_calibrated_control_exact_pages_and_checksums() -> None:
    artifact = _artifact()
    validate_matrix_artifact(artifact, load_experiment_configs(CONFIGS))

    broken = deepcopy(artifact)
    broken["baseline_calibration"]["rust_control_matches_task2_best"] = False
    with pytest.raises(ValueError, match="calibration"):
        validate_matrix_artifact(broken, load_experiment_configs(CONFIGS))

    broken = deepcopy(artifact)
    broken["candidates"][1]["records"][0]["character_edits"] += 1
    broken["candidates"][1]["aggregate"] = aggregate_matrix_records(
        broken["candidates"][1]["records"]
    )
    broken["candidates"][1]["strata"]["low-contrast"] = aggregate_matrix_records(
        broken["candidates"][1]["records"]
    )
    for kind in ("historical-old-print", "modern-government"):
        broken["candidates"][1]["document_types"][kind] = aggregate_matrix_records(
            [
                record
                for record in broken["candidates"][1]["records"]
                if record["document_type"] == kind
            ]
        )
    with pytest.raises(ValueError, match="transfer"):
        validate_matrix_artifact(broken, load_experiment_configs(CONFIGS))

    broken = deepcopy(artifact)
    broken["candidates"][1]["records"][0]["transform_sha256"] = "bad"
    with pytest.raises(ValueError, match="transform checksum"):
        validate_matrix_artifact(broken, load_experiment_configs(CONFIGS))

    broken = deepcopy(artifact)
    broken["candidates"][1]["records"][0]["transform_sha256"] = "b" * 64
    with pytest.raises(ValueError, match="transform checksum"):
        validate_matrix_artifact(broken, load_experiment_configs(CONFIGS))

    broken = deepcopy(artifact)
    broken["candidates"][1]["records"][0]["input_sha256s"] = []
    with pytest.raises(ValueError, match="input checksum"):
        validate_matrix_artifact(broken, load_experiment_configs(CONFIGS))

    broken = deepcopy(artifact)
    broken["candidates"][0]["records"][0]["source_sha256"] = "bad"
    with pytest.raises(ValueError, match="source checksum"):
        validate_matrix_artifact(broken, load_experiment_configs(CONFIGS))


def test_report_ranks_tuning_cer_and_labels_strata_and_dpi_limit_honestly() -> None:
    artifact = _artifact()
    artifact["candidates"][2]["records"][0]["character_edits"] = 1
    artifact["candidates"][2]["aggregate"] = aggregate_matrix_records(
        artifact["candidates"][2]["records"]
    )
    artifact["candidates"][2]["strata"]["low-contrast"] = aggregate_matrix_records(
        artifact["candidates"][2]["records"]
    )
    modern = [
        row
        for row in artifact["candidates"][2]["records"]
        if row["document_type"] == "modern-government"
    ]
    artifact["candidates"][2]["document_types"]["modern-government"] = (
        aggregate_matrix_records(modern)
    )

    report = render_matrix_report(artifact)
    ranked = report.split("## Ranked overall tuning measurements", 1)[1].split(
        "## Exact one-factor configurations", 1
    )[0]

    assert ranked.index("`dpi-hint-400`") < ranked.index("`control`")
    assert "Current best baseline: **12.2174% CER**" in report
    assert "modern-government" in report
    assert "historical-old-print" in report
    assert "small strata are descriptive" in report
    assert "DPI hint" in report
    assert "not a 300/400 PDF rerender comparison" in report
    assert "No holdout result" in report
    assert "Transfer gap: **" in report
    assert "actual Rust control" in report
    assert "not directly production-equivalent" in report
    assert "CER change vs Rust control" in report
    assert "CER change vs direct transfer" in report
    document_section = report.split("## Document-type strata", 1)[1].split(
        "## Overlapping difficulty strata", 1
    )[0]
    difficulty_section = report.split("## Overlapping difficulty strata", 1)[1].split(
        "## Raw additive counts", 1
    )[0]
    for section in (document_section, difficulty_section):
        assert "Median s/page" in section
        assert "p95 s/page" in section
        assert "Peak RSS MiB" in section
        assert "Failures" in section
    assert "Tesseract binary SHA-256" in report
    assert artifact["provenance"]["tesseract_binary_sha256"] in report
    assert "recognized_text" not in report
    assert "reference text" not in report.lower()
