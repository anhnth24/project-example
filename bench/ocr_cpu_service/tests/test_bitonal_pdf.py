from __future__ import annotations

import copy
import hashlib
import json
import sys
from pathlib import Path
from unittest.mock import patch

import pytest

sys.path.insert(0, str(Path(__file__).parents[1]))

from benchmark.candidates import CommandCandidateSpec
from experiments.bitonal_pdf import (  # noqa: E402
    CALIBRATION_RUN_ROOT,
    EXPECTED_CANDIDATE_IDS,
    _DIAGNOSTIC_FIELDS,
    _PLAN_LIMITS,
    allocate_calibration_work_dir,
    build_calibration_candidates,
    load_calibration_config,
    page_diagnostics,
    recognize_calibration_page,
    reference_disagreement_counts,
    release_calibration_work_dir,
    render_calibration_pages,
    strip_for_disagreement,
    validate_calibration_artifact,
)

SERVICE_ROOT = Path(__file__).parents[1]
CONFIGS = SERVICE_ROOT / "experiments" / "bitonal-configs.json"
ROOT = SERVICE_ROOT.parents[1]


def _checksum() -> str:
    return "a" * 64


def _bindings_from_provenance(
    provenance: dict[str, object],
    *,
    tessdata_role: str,
    render_sha256: str,
) -> dict[str, object]:
    return {
        "source_sha256": provenance["source_sha256"],
        "config_sha256": provenance["config_sha256"],
        "binary_sha256": provenance["binary_sha256"],
        "pdfium_sha256": provenance["pdfium_sha256"],
        "toolchain_sha256": provenance["toolchain_sha256"],
        "tessdata_sha256": provenance["tessdata_sha256"][tessdata_role],
        "render_sha256": render_sha256,
    }


def _record(
    candidate_id: str,
    page_number: int,
    *,
    provenance: dict[str, object],
    tessdata_role: str,
    success: bool = True,
    render_sha256: str,
) -> dict[str, object]:
    record: dict[str, object] = {
        "candidate_id": candidate_id,
        "page_number": page_number,
        "success": success,
        "bindings": _bindings_from_provenance(
            provenance,
            tessdata_role=tessdata_role,
            render_sha256=render_sha256,
        ),
        "elapsed_seconds": 1.0 if success else 0.0,
        "peak_rss_bytes": 100 if success else 0,
        "resource_limit_violation": False,
    }
    if success:
        if page_number <= 20:
            record.update(
                character_edits=2,
                reference_characters=20,
                word_edits=1,
                reference_words=4,
            )
        else:
            record["diagnostics"] = {
                "digit_sequence_count": 3,
                "digit_sequence_checksum": _checksum(),
                "legal_identifier_count": 1 if page_number == 450 else 0,
                "non_whitespace_character_count": 120,
                "suspicious_character_count": 0,
                "accent_proxy_counts": (
                    {
                        "latin-o-for-o-with-hook": 0,
                        "latin-u-for-u-with-hook": 0,
                        "latin-a-for-a-with-breve": 0,
                    }
                    if page_number == 60
                    else {}
                ),
            }
    else:
        record["error_kind"] = "timeout"
    return record


def valid_artifact() -> dict[str, object]:
    config = load_calibration_config(CONFIGS)
    host = {
        "platform": "linux",
        "architecture": "x86_64",
        "logical_cpus": 8,
        "memory_bytes": 1000,
        "max_rss_bytes": config["limits"]["max_rss_bytes"],
        "max_rss_enforcement": "measured_gate_only_not_os_enforced",
    }
    toolchain = {
        "cargo": "cargo 1.0",
        "pypdfium2": "5.0.0",
        "python": "3.12.0",
        "tesseract": "tesseract 5.3.4",
    }
    provenance = {
        "source_sha256": config["source"]["expected_sha256"],
        "config_sha256": _checksum(),
        "binary_sha256": _checksum(),
        "tessdata_sha256": {
            "system": {"vie": _checksum(), "eng": _checksum()},
            "best": {"vie": _checksum(), "eng": _checksum()},
        },
        "pdfium_sha256": _checksum(),
        "host_sha256": hashlib.sha256(
            json.dumps(host, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest(),
        "toolchain_sha256": hashlib.sha256(
            json.dumps(
                toolchain, ensure_ascii=False, sort_keys=True, separators=(",", ":")
            ).encode()
        ).hexdigest(),
    }
    shared_render = _checksum()
    render_hashes = {
        str(page_number): shared_render
        for page_number in list(range(1, 21)) + [60, 450]
    }
    candidates = []
    for candidate in config["candidates"]:
        candidates.append(
            {
                "id": candidate["id"],
                "mode": candidate["mode"],
                "tessdata": candidate["tessdata"],
                "langs": candidate["langs"],
                "argv": [
                    "{fileconv}",
                    "one",
                    "{input}",
                    "--lang",
                    candidate["langs"],
                ],
                "environment_variable_names": candidate["environment_variable_names"],
                "aggregate": {
                    "pages": 22,
                    "successes": 22,
                    "failures": 0,
                    "latency_seconds": {"median": 1.0, "total": 22.0},
                    "peak_rss_bytes": 100,
                    "resource_limit_violations": 0,
                    "raw_counts": {
                        "character_edits": 40,
                        "reference_characters": 400,
                        "word_edits": 20,
                        "reference_words": 80,
                    },
                },
            }
        )
    records = [
        _record(
            candidate_id,
            page_number,
            provenance=provenance,
            tessdata_role=next(
                item["tessdata"]
                for item in config["candidates"]
                if item["id"] == candidate_id
            ),
            render_sha256=shared_render,
        )
        for candidate_id in EXPECTED_CANDIDATE_IDS
        for page_number in list(range(1, 21)) + [60, 450]
    ]
    return {
        "schema_version": 1,
        "split": "calibration",
        "page_count": 22,
        "provenance": provenance,
        "host": host,
        "toolchain": toolchain,
        "limits": dict(_PLAN_LIMITS),
        "access": {
            "approved_pages_opened": 22,
            "holdout_pages_opened": 0,
            "rendered_pages": 22,
            "ocr_executions": 88,
        },
        "render_hashes": render_hashes,
        "candidates": candidates,
        "records": records,
    }


def test_calibration_config_loads_four_exact_candidates() -> None:
    config = load_calibration_config(CONFIGS)
    assert tuple(candidate["id"] for candidate in config["candidates"]) == (
        EXPECTED_CANDIDATE_IDS
    )


def test_calibration_rejects_holdout_or_unapproved_pages() -> None:
    payload = valid_artifact()
    payload["records"][0]["page_number"] = 21
    with pytest.raises(ValueError, match="approved calibration pages"):
        validate_calibration_artifact(payload)


def test_calibration_rejects_invalid_candidate_ids() -> None:
    payload = valid_artifact()
    payload["candidates"][0]["id"] = "other"
    with pytest.raises(ValueError, match="candidate IDs are invalid"):
        validate_calibration_artifact(payload)


def test_calibration_rejects_missing_provenance_checksums() -> None:
    payload = valid_artifact()
    del payload["provenance"]["binary_sha256"]
    with pytest.raises(ValueError, match="missing field"):
        validate_calibration_artifact(payload)


def test_calibration_rejects_recognized_text_fields() -> None:
    payload = valid_artifact()
    payload["records"][0]["recognized_text"] = "secret"
    with pytest.raises(ValueError, match="forbidden field"):
        validate_calibration_artifact(payload)


def test_calibration_rejects_reference_text_fields() -> None:
    payload = valid_artifact()
    payload["records"][0]["reference_text"] = "secret"
    with pytest.raises(ValueError, match="forbidden field"):
        validate_calibration_artifact(payload)


def test_calibration_rejects_stdout_and_stderr_fields() -> None:
    payload = valid_artifact()
    payload["records"][0]["stdout"] = "secret"
    with pytest.raises(ValueError, match="forbidden field"):
        validate_calibration_artifact(payload)


def test_calibration_rejects_environment_values() -> None:
    payload = valid_artifact()
    payload["candidates"][0]["environment"] = {"FILECONV_TESSDATA": "/secret"}
    with pytest.raises(ValueError, match="forbidden field"):
        validate_calibration_artifact(payload)


def test_calibration_rejects_ground_truth_alias() -> None:
    payload = valid_artifact()
    payload["records"][0]["ground_truth"] = "secret"
    with pytest.raises(ValueError, match="forbidden field"):
        validate_calibration_artifact(payload)


def test_calibration_rejects_recognized_output_alias() -> None:
    payload = valid_artifact()
    payload["records"][0]["recognized_output"] = "secret"
    with pytest.raises(ValueError, match="forbidden field"):
        validate_calibration_artifact(payload)


def test_calibration_rejects_env_alias() -> None:
    payload = valid_artifact()
    payload["records"][0]["env"] = "secret"
    with pytest.raises(ValueError, match="forbidden field"):
        validate_calibration_artifact(payload)


def test_calibration_rejects_cer_field() -> None:
    payload = valid_artifact()
    payload["records"][0]["cer"] = 0.1
    with pytest.raises(ValueError, match="forbidden field"):
        validate_calibration_artifact(payload)


def test_calibration_rejects_unknown_artifact_keys() -> None:
    payload = valid_artifact()
    payload["extra"] = True
    with pytest.raises(ValueError, match="unknown field at artifact"):
        validate_calibration_artifact(payload)


def test_calibration_rejects_extra_candidate_fields() -> None:
    payload = valid_artifact()
    payload["candidates"][0]["label"] = "extra"
    with pytest.raises(ValueError, match="unknown field at artifact.candidate"):
        validate_calibration_artifact(payload)


def test_calibration_validates_exact_candidate_semantics() -> None:
    payload = valid_artifact()
    payload["candidates"][0]["mode"] = "preserve-near-bitonal"
    with pytest.raises(ValueError, match="semantics drifted"):
        validate_calibration_artifact(payload)


def test_calibration_requires_shared_render_hash_per_page() -> None:
    payload = valid_artifact()
    payload["records"][1]["bindings"]["render_sha256"] = "b" * 64
    with pytest.raises(ValueError, match="render binding is inconsistent"):
        validate_calibration_artifact(payload)


def test_calibration_binds_record_provenance_to_top_level() -> None:
    payload = valid_artifact()
    payload["records"][0]["bindings"]["binary_sha256"] = "b" * 64
    with pytest.raises(ValueError, match="binary binding is inconsistent"):
        validate_calibration_artifact(payload)


def test_calibration_rejects_duplicate_candidate_page_records() -> None:
    payload = valid_artifact()
    payload["records"].append(copy.deepcopy(payload["records"][0]))
    with pytest.raises(ValueError, match="duplicate candidate-page record"):
        validate_calibration_artifact(payload)


def test_calibration_rejects_missing_candidate_page_records() -> None:
    payload = valid_artifact()
    payload["records"] = payload["records"][:-1]
    with pytest.raises(ValueError, match="cardinality is invalid"):
        validate_calibration_artifact(payload)


def test_calibration_rejects_unbounded_timeout() -> None:
    payload = valid_artifact()
    payload["limits"]["timeout_seconds_per_page"] = 0
    with pytest.raises(ValueError, match="must match approved constants"):
        validate_calibration_artifact(payload)


def test_calibration_rejects_unbounded_output_limit() -> None:
    payload = valid_artifact()
    payload["limits"]["max_output_bytes_per_stream"] = -1
    with pytest.raises(ValueError, match="must match approved constants"):
        validate_calibration_artifact(payload)


def test_calibration_rejects_unbounded_rss_limit() -> None:
    payload = valid_artifact()
    payload["limits"]["max_rss_bytes"] = 0
    with pytest.raises(ValueError, match="must match approved constants"):
        validate_calibration_artifact(payload)


def test_calibration_rejects_unbounded_process_tree_sampling() -> None:
    payload = valid_artifact()
    payload["limits"]["process_tree_sample_interval_ms"] = 0
    with pytest.raises(ValueError, match="must match approved constants"):
        validate_calibration_artifact(payload)


def test_calibration_accepts_valid_fixture() -> None:
    validate_calibration_artifact(valid_artifact())


def test_calibration_accepts_bounded_failure_record() -> None:
    payload = valid_artifact()
    payload["records"][0] = _record(
        EXPECTED_CANDIDATE_IDS[0],
        1,
        provenance=payload["provenance"],
        tessdata_role="system",
        success=False,
        render_sha256=str(payload["render_hashes"]["1"]),
    )
    payload["candidates"][0]["aggregate"]["successes"] = 21
    payload["candidates"][0]["aggregate"]["failures"] = 1
    validate_calibration_artifact(payload)


def test_calibration_rejects_failure_record_with_text() -> None:
    payload = valid_artifact()
    failure = _record(
        EXPECTED_CANDIDATE_IDS[0],
        1,
        provenance=payload["provenance"],
        tessdata_role="system",
        success=False,
        render_sha256=str(payload["render_hashes"]["1"]),
    )
    failure["recognized_text"] = "secret"
    payload["records"][0] = failure
    with pytest.raises(ValueError, match="forbidden field"):
        validate_calibration_artifact(payload)


def test_calibration_rejects_success_record_without_metrics() -> None:
    payload = valid_artifact()
    del payload["records"][0]["character_edits"]
    with pytest.raises(ValueError, match="missing field at record"):
        validate_calibration_artifact(payload)


def test_strip_for_disagreement_removes_markdown_and_page_numbers() -> None:
    text = "# Tiêu đề\n\n12\n\nNội dung *quan trọng*."
    assert strip_for_disagreement(text) == "Tiêu đề Nội dung quan trọng ."


def test_reference_disagreement_returns_counts_not_cer_label() -> None:
    counts = reference_disagreement_counts("một hai ba", "mot hai ba")
    assert set(counts) == {
        "character_edits",
        "reference_characters",
        "word_edits",
        "reference_words",
    }
    assert "cer" not in counts


def test_reference_disagreement_page_number_decoration_does_not_inflate_counts() -> None:
    plain = "một hai ba"
    decorated = "# 12\n\nmột hai ba"
    with_both_cleaned = reference_disagreement_counts(plain, decorated)
    assert with_both_cleaned["character_edits"] == 0


def test_page_diagnostics_include_accent_proxy_counts_from_note() -> None:
    config = load_calibration_config(CONFIGS)
    note = (
        "Page 60 may show thong/tuong/cuong, quy/luat/thu, and ban hanh/cap nhat "
        "accent issues."
    )
    diagnostics = page_diagnostics(
        "thong tu quy dinh",
        page_number=60,
        config=config,
        note_text=note,
    )
    assert set(diagnostics) == set(_DIAGNOSTIC_FIELDS)
    assert diagnostics["accent_proxy_counts"]["latin-o-for-o-with-hook"] >= 1


def test_page_diagnostics_require_note_context_for_accent_proxies() -> None:
    config = load_calibration_config(CONFIGS)
    with pytest.raises(ValueError, match="missing from the supplied note"):
        page_diagnostics(
            "thong tu",
            page_number=60,
            config=config,
            note_text="unrelated note",
        )


def test_calibration_work_dir_is_created_under_fixed_root() -> None:
    path = allocate_calibration_work_dir()
    try:
        assert path.is_dir()
        assert path.resolve().is_relative_to(CALIBRATION_RUN_ROOT.resolve())
    finally:
        release_calibration_work_dir(path)


def test_calibration_work_dir_cleanup_on_render_failure(tmp_path: Path) -> None:
    work_dir = allocate_calibration_work_dir()
    config = load_calibration_config(CONFIGS)
    pdf_path = tmp_path / "official.pdf"
    pdf_path.write_bytes(b"%PDF-1.4\n")

    def fail_render(*_args, **_kwargs):
        raise ValueError("render failed")

    try:
        with patch(
            "test_bitonal_pdf.render_calibration_pages",
            side_effect=fail_render,
        ):
            with pytest.raises(ValueError, match="render failed"):
                render_calibration_pages(pdf_path, work_dir, config=config)
    finally:
        release_calibration_work_dir(work_dir)
    assert not work_dir.exists()


def test_run_calibration_cleans_work_dir_after_inference_failures(tmp_path: Path) -> None:
    from argparse import Namespace

    from experiments.bitonal_pdf import run_calibration

    work_dirs: list[Path] = []
    original_allocate = allocate_calibration_work_dir

    def track_allocate() -> Path:
        path = original_allocate()
        work_dirs.append(path)
        return path

    args = Namespace(
        config=CONFIGS,
        sources=SERVICE_ROOT / "corpus" / "sources.json",
        pdf=tmp_path / "official.pdf",
        reference=tmp_path / "reference.md",
        note=tmp_path / "note.md",
        fileconv=Path("/bin/true"),
        pdfium_lib=tmp_path,
        system_tessdata=tmp_path,
        best_tessdata=tmp_path,
        output=tmp_path / "out.json",
    )
    args.pdf.write_bytes(b"%PDF-1.4\n")
    args.reference.write_text(
        "\n".join(f"<!-- page {page} -->\nmột hai ba" for page in range(1, 21)),
        encoding="utf-8",
    )
    args.note.write_text(
        "thong tuong cuong quy luat thu ban hanh cap nhat",
        encoding="utf-8",
    )
    (tmp_path / "vie.traineddata").write_bytes(b"vie")
    (tmp_path / "eng.traineddata").write_bytes(b"eng")
    (tmp_path / "libpdfium.so").write_bytes(b"pdfium")
    rendered = {
        page_number: (tmp_path / f"page-{page_number}.png", "a" * 64)
        for page_number in list(range(1, 21)) + [60, 450]
    }
    for page_path, _ in rendered.values():
        page_path.write_bytes(b"png")

    with (
        patch("experiments.bitonal_pdf.allocate_calibration_work_dir", track_allocate),
        patch(
            "experiments.bitonal_pdf.render_calibration_pages",
            return_value=rendered,
        ),
        patch(
            "experiments.bitonal_pdf.recognize_calibration_page",
            return_value=(None, {"elapsed_seconds": 0.0, "peak_rss_bytes": 0, "resource_limit_violation": False}, "timeout"),
        ),
        patch("experiments.bitonal_pdf._verify_official_pdf"),
        patch(
            "experiments.bitonal_pdf._host_description",
            return_value={
                "platform": "linux",
                "architecture": "x86_64",
                "logical_cpus": 8,
                "memory_bytes": 1000,
                "max_rss_bytes": _PLAN_LIMITS["max_rss_bytes"],
                "max_rss_enforcement": "measured_gate_only_not_os_enforced",
            },
        ),
        patch(
            "experiments.bitonal_pdf._toolchain_description",
            return_value={
                "cargo": "cargo 1.0",
                "pypdfium2": "5.0.0",
                "python": "3.12.0",
                "tesseract": "tesseract 5.3.4",
            },
        ),
    ):
        artifact = run_calibration(args)
    assert all(record["error_kind"] == "timeout" for record in artifact["records"])
    assert len(work_dirs) == 1
    assert not work_dirs[0].exists()


def test_run_calibration_cleans_work_dir_on_render_failure(tmp_path: Path) -> None:
    from argparse import Namespace

    from experiments.bitonal_pdf import run_calibration

    work_dirs: list[Path] = []
    original_allocate = allocate_calibration_work_dir

    def track_allocate() -> Path:
        path = original_allocate()
        work_dirs.append(path)
        return path

    args = Namespace(
        config=CONFIGS,
        sources=SERVICE_ROOT / "corpus" / "sources.json",
        pdf=tmp_path / "official.pdf",
        reference=tmp_path / "reference.md",
        note=tmp_path / "note.md",
        fileconv=Path("/bin/true"),
        pdfium_lib=tmp_path,
        system_tessdata=tmp_path,
        best_tessdata=tmp_path,
        output=tmp_path / "out.json",
    )
    args.pdf.write_bytes(b"%PDF-1.4\n")
    args.reference.write_text(
        "\n".join(f"<!-- page {page} -->\nmột hai ba" for page in range(1, 21)),
        encoding="utf-8",
    )
    args.note.write_text("note", encoding="utf-8")
    (tmp_path / "vie.traineddata").write_bytes(b"vie")
    (tmp_path / "eng.traineddata").write_bytes(b"eng")
    (tmp_path / "libpdfium.so").write_bytes(b"pdfium")

    with (
        patch("experiments.bitonal_pdf.allocate_calibration_work_dir", track_allocate),
        patch(
            "experiments.bitonal_pdf.render_calibration_pages",
            side_effect=ValueError("render failed"),
        ),
        patch("experiments.bitonal_pdf._verify_official_pdf"),
    ):
        with pytest.raises(ValueError, match="render failed"):
            run_calibration(args)
    assert len(work_dirs) == 1
    assert not work_dirs[0].exists()


def test_calibration_uses_bounded_worker_contract(tmp_path: Path) -> None:
    from dataclasses import replace

    recognizer = tmp_path / "recognizer.py"
    recognizer.write_text(
        "from pathlib import Path\n"
        "import sys\n"
        "print(Path(sys.argv[1]).read_text(encoding='utf-8'))\n",
        encoding="utf-8",
    )
    page_path = tmp_path / "page.png"
    page_path.write_text("xin chào", encoding="utf-8")
    config = load_calibration_config(CONFIGS)
    candidates = build_calibration_candidates(
        config,
        fileconv=Path(sys.executable),
        system_tessdata=tmp_path,
        best_tessdata=tmp_path,
        pdfium_lib=tmp_path,
    )
    spec = CommandCandidateSpec(
        id=candidates[0].id,
        label=candidates[0].id,
        argv=(sys.executable, str(recognizer), "{input}"),
        environment=candidates[0].spec.environment,
        provenance={},
    )
    candidate = replace(candidates[0], spec=spec)
    text, resource, error_kind = recognize_calibration_page(
        candidate,
        page_path=page_path,
        page_number=1,
        timeout_seconds=5.0,
        max_output_bytes=4096,
        max_rss_bytes=1024 * 1024 * 1024,
    )
    assert error_kind is None
    assert text.strip() == "xin chào"
    assert resource["peak_rss_bytes"] >= 0


def test_sandbox_excludes_fileconv_ocr_preprocess_mode() -> None:
    sandbox = (ROOT / "crates/server/src/workers/sandbox.rs").read_text(encoding="utf-8")
    assert "FILECONV_OCR_PREPROCESS_MODE" not in sandbox
