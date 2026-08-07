"""Text normalization and edit-distance metrics for OCR evidence."""

from __future__ import annotations

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
