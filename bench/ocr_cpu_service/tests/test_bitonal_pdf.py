from __future__ import annotations

import copy
import hashlib
import json
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parents[1]))

from experiments.bitonal_pdf import (  # noqa: E402
    EXPECTED_CANDIDATE_IDS,
    _DIAGNOSTIC_FIELDS,
    load_calibration_config,
    page_diagnostics,
    reference_disagreement_counts,
    strip_reference_for_disagreement,
    validate_calibration_artifact,
)

SERVICE_ROOT = Path(__file__).parents[1]
CONFIGS = SERVICE_ROOT / "experiments" / "bitonal-configs.json"


def _checksum() -> str:
    return "a" * 64


def _record(candidate_id: str, page_number: int) -> dict[str, object]:
    record: dict[str, object] = {
        "candidate_id": candidate_id,
        "page_number": page_number,
        "mode": "legacy",
        "langs": "vie+eng",
        "render_sha256": _checksum(),
        "success": True,
        "error_kind": None,
        "elapsed_seconds": 1.0,
        "peak_rss_bytes": 100,
        "resource_limit_violation": False,
    }
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
                {"latin-o-for-o-with-hook": 0} if page_number == 60 else {}
            ),
        }
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
        "source_sha256": _checksum(),
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
    candidates = []
    for candidate in config["candidates"]:
        candidates.append(
            {
                "id": candidate["id"],
                "mode": candidate["mode"],
                "tessdata": candidate["tessdata"],
                "langs": candidate["langs"],
                "argv": [
                    "/tmp/fileconv",
                    "one",
                    "{input}",
                    "--lang",
                    candidate["langs"],
                ],
                "environment_variable_names": ["FILECONV_PDFIUM_LIB", "FILECONV_TESSDATA"],
                "provenance": {
                    "binary_sha256": provenance["binary_sha256"],
                    "toolchain_sha256": provenance["toolchain_sha256"],
                    "mode": candidate["mode"],
                    "tessdata": {
                        "role": candidate["tessdata"],
                        "sha256": provenance["tessdata_sha256"][
                            "system" if candidate["tessdata"] == "system" else "best"
                        ],
                    },
                    "invocation": "fileconv one <rendered-page.png>",
                },
                "aggregate": {"pages": 22, "successes": 22, "failures": 0},
            }
        )
    records = [
        _record(candidate_id, page_number)
        for candidate_id in EXPECTED_CANDIDATE_IDS
        for page_number in list(range(1, 21)) + [60, 450]
    ]
    for record in records:
        candidate = next(
            item for item in candidates if item["id"] == record["candidate_id"]
        )
        record["mode"] = candidate["mode"]
        record["langs"] = candidate["langs"]
    return {
        "schema_version": 1,
        "split": "calibration",
        "page_count": 22,
        "provenance": provenance,
        "host": host,
        "toolchain": toolchain,
        "limits": config["limits"],
        "access": {
            "approved_pages_opened": 22,
            "holdout_pages_opened": 0,
            "rendered_pages": 22,
            "ocr_executions": 88,
        },
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
    with pytest.raises(ValueError, match="checksum set is incomplete"):
        validate_calibration_artifact(payload)


def test_calibration_rejects_recognized_text_fields() -> None:
    payload = valid_artifact()
    payload["records"][0]["recognized_text"] = "secret"
    with pytest.raises(ValueError, match="recognized text field is forbidden"):
        validate_calibration_artifact(payload)


def test_calibration_rejects_reference_text_fields() -> None:
    payload = valid_artifact()
    payload["records"][0]["reference_text"] = "secret"
    with pytest.raises(ValueError, match="recognized text field is forbidden"):
        validate_calibration_artifact(payload)


def test_calibration_rejects_stdout_and_stderr_fields() -> None:
    payload = valid_artifact()
    payload["records"][0]["stdout"] = "secret"
    with pytest.raises(ValueError, match="recognized text field is forbidden"):
        validate_calibration_artifact(payload)


def test_calibration_rejects_environment_values() -> None:
    payload = valid_artifact()
    payload["candidates"][0]["environment"] = {"FILECONV_TESSDATA": "/secret"}
    with pytest.raises(ValueError, match="environment values are forbidden"):
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
    with pytest.raises(ValueError, match="limits must remain positive and bounded"):
        validate_calibration_artifact(payload)


def test_calibration_rejects_unbounded_output_limit() -> None:
    payload = valid_artifact()
    payload["limits"]["max_output_bytes_per_stream"] = -1
    with pytest.raises(ValueError, match="limits must remain positive and bounded"):
        validate_calibration_artifact(payload)


def test_calibration_rejects_unbounded_rss_limit() -> None:
    payload = valid_artifact()
    payload["limits"]["max_rss_bytes"] = 0
    with pytest.raises(ValueError, match="limits must remain positive and bounded"):
        validate_calibration_artifact(payload)


def test_calibration_rejects_unbounded_process_tree_sampling() -> None:
    payload = valid_artifact()
    payload["limits"]["process_tree_sample_interval_ms"] = 0
    with pytest.raises(ValueError, match="sampling interval must remain bounded"):
        validate_calibration_artifact(payload)


def test_calibration_accepts_valid_fixture() -> None:
    validate_calibration_artifact(valid_artifact())


def test_strip_reference_removes_markdown_and_page_numbers() -> None:
    text = "# Tiêu đề\n\n12\n\nNội dung *quan trọng*."
    assert strip_reference_for_disagreement(text) == "Tiêu đề Nội dung quan trọng ."


def test_reference_disagreement_returns_counts_not_cer_label() -> None:
    counts = reference_disagreement_counts("một hai ba", "mot hai ba")
    assert set(counts) == {
        "character_edits",
        "reference_characters",
        "word_edits",
        "reference_words",
    }
    assert "cer" not in counts


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
