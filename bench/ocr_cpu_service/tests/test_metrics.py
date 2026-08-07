from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parents[1]))

from benchmark.metrics import (  # noqa: E402
    cer,
    error_counts,
    normalize_for_metric,
    wer,
)


def test_cer_normalizes_nfc_and_whitespace() -> None:
    assert cer("Cộng  hòa", "Co\u0323\u0302ng hòa") == 0.0


def test_empty_reference_policy_is_explicit() -> None:
    assert cer("", "") == 0.0
    assert cer("", "x") == 1.0
    assert wer("", "") == 0.0
    assert wer("", "x") == 1.0


def test_metric_normalization_preserves_case_and_content() -> None:
    assert normalize_for_metric("  Cộng\n\thòa  ") == "Cộng hòa"
    assert cer("Việt Nam", "việt Nam") > 0.0


def test_wer_counts_word_edits_after_normalization() -> None:
    assert wer("Cộng hòa Việt Nam", "Cộng hòa Nam") == 0.25


def test_error_counts_support_micro_averaging_without_storing_text() -> None:
    counts = error_counts("một hai ba", "một ba")

    assert counts.reference_characters == 10
    assert counts.character_edits == 4
    assert counts.reference_words == 3
    assert counts.word_edits == 1
