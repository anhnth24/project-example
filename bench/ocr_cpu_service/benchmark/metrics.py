"""Text normalization and edit-distance metrics for OCR evidence."""

from __future__ import annotations

import re
import unicodedata
from collections.abc import Sequence
from dataclasses import dataclass
from typing import TypeVar

T = TypeVar("T")


@dataclass(frozen=True, slots=True)
class ErrorCounts:
    character_edits: int
    reference_characters: int
    word_edits: int
    reference_words: int


@dataclass(frozen=True, slots=True)
class ReadingOrderCounts:
    expected_anchors: int
    observed_anchors: int
    comparable_pairs: int
    violations: int
    missing_anchors: int


def normalize_for_metric(text: str) -> str:
    """Normalize canonically equivalent text and collapse whitespace."""
    return " ".join(unicodedata.normalize("NFC", text).split())


def _distance(reference: Sequence[T], hypothesis: Sequence[T]) -> int:
    if len(reference) > len(hypothesis):
        reference, hypothesis = hypothesis, reference
    previous = list(range(len(reference) + 1))
    for hypothesis_item in hypothesis:
        current = [previous[0] + 1]
        for index, reference_item in enumerate(reference, start=1):
            current.append(
                min(
                    current[-1] + 1,
                    previous[index] + 1,
                    previous[index - 1]
                    + (reference_item != hypothesis_item),
                )
            )
        previous = current
    return previous[-1]


def _rate(reference: Sequence[T], hypothesis: Sequence[T]) -> float:
    if not reference:
        return 0.0 if not hypothesis else 1.0
    return _distance(reference, hypothesis) / len(reference)


def cer(reference: str, hypothesis: str) -> float:
    """Return character error rate after shared metric normalization."""
    normalized_reference = normalize_for_metric(reference)
    normalized_hypothesis = normalize_for_metric(hypothesis)
    return _rate(normalized_reference, normalized_hypothesis)


def wer(reference: str, hypothesis: str) -> float:
    """Return word error rate after shared metric normalization."""
    normalized_reference = normalize_for_metric(reference).split()
    normalized_hypothesis = normalize_for_metric(hypothesis).split()
    return _rate(normalized_reference, normalized_hypothesis)


def error_counts(reference: str, hypothesis: str) -> ErrorCounts:
    """Return additive edit counts suitable for corpus-level micro averages."""
    normalized_reference = normalize_for_metric(reference)
    normalized_hypothesis = normalize_for_metric(hypothesis)
    reference_words = normalized_reference.split()
    hypothesis_words = normalized_hypothesis.split()
    return ErrorCounts(
        character_edits=_distance(
            normalized_reference, normalized_hypothesis
        ),
        reference_characters=len(normalized_reference),
        word_edits=_distance(reference_words, hypothesis_words),
        reference_words=len(reference_words),
    )


def reading_order_violations(
    expected_anchors: Sequence[str], hypothesis: str
) -> ReadingOrderCounts:
    """Count pairwise inversions among uniquely reviewed anchor tokens."""
    expected = tuple(expected_anchors)
    if len(expected) != len(set(expected)) or any(not item for item in expected):
        raise ValueError("reading-order anchors must be non-empty and unique")
    normalized = normalize_for_metric(hypothesis)
    observed: list[tuple[int, int]] = []
    for expected_index, anchor in enumerate(expected):
        match = re.search(
            rf"(?<!\w){re.escape(anchor)}(?!\w)",
            normalized,
        )
        if match is not None:
            observed.append((expected_index, match.start()))
    observed.sort(key=lambda item: item[1])
    violations = sum(
        left_expected > right_expected
        for index, (left_expected, _) in enumerate(observed)
        for right_expected, _ in observed[index + 1 :]
    )
    observed_count = len(observed)
    return ReadingOrderCounts(
        expected_anchors=len(expected),
        observed_anchors=observed_count,
        comparable_pairs=observed_count * (observed_count - 1) // 2,
        violations=violations,
        missing_anchors=len(expected) - observed_count,
    )
