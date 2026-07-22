"""Strict claimsLiveVerticalSlice coverage rules."""

from __future__ import annotations

from typing import Any, Iterable

from .evidence import CaseResult


def required_case_ids(suite: dict[str, Any]) -> list[tuple[str, str]]:
    """Return ordered (matrix, id) for every required coverage case.

    Optional-model / optional-runtime formats are NOT required coverage.
    Every security, adversarial, and fault case is required.
    """
    out: list[tuple[str, str]] = []
    for fmt in suite.get("formats") or []:
        if fmt.get("requirement") == "required":
            out.append(("format", fmt["id"]))
    for sec in suite.get("security") or []:
        out.append(("security", sec["id"]))
    for adv in suite.get("adversarial") or []:
        out.append(("adversarial", adv["id"]))
    for fault in suite.get("fault") or []:
        out.append(("fault", fault["id"]))
    return out


def evaluate_claims_live_vertical_slice(
    suite: dict[str, Any],
    cases: Iterable[CaseResult],
) -> tuple[bool, list[str]]:
    """True only if every required case appears exactly once and status==pass.

    Any blocked / fail / optional_unavailable / unknown / duplicate / missing
    required case keeps claims false. Optional cases never satisfy required coverage.
    """
    required = required_case_ids(suite)
    required_ids = {cid for _, cid in required}
    required_matrix = {cid: matrix for matrix, cid in required}

    seen: dict[str, CaseResult] = {}
    errors: list[str] = []
    for case in cases:
        if case.id in required_ids:
            if case.id in seen:
                errors.append(f"duplicate required case: {case.id}")
            seen[case.id] = case

    for cid in sorted(required_ids):
        if cid not in seen:
            errors.append(f"missing required case: {cid}")
            continue
        case = seen[cid]
        expect_matrix = required_matrix[cid]
        if case.matrix != expect_matrix:
            errors.append(
                f"{cid}: matrix mismatch (got {case.matrix}, want {expect_matrix})"
            )
        if case.status != "pass":
            errors.append(f"{cid}: required case status={case.status} (want pass)")

    return (not errors, errors)
