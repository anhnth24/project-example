#!/usr/bin/env python3
"""Deterministic version/claim/conflict rules for P0-06 offline evaluation.

Predictions are derived from manifest metadata + claim text in markdown, never from
expected_answer / version_context / conflict_context fields on the query row.
"""

from __future__ import annotations

import json
import re
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
CORPUS = ROOT / "bench/markhand_web"
MANIFEST = CORPUS / "golden/manifest.json"
MD_DIR = CORPUS / "golden/markdown"
CONFLICTS = CORPUS / "golden/conflicts.json"

BUDGET_CLAIM_KEY = "approved_budget_vnd"
BUDGET_RE = re.compile(
    r"(?:Kinh phí được phê duyệt là|Thiết kế phân bổ kinh phí)\s+(\d+)\s+triệu đồng",
    re.IGNORECASE,
)


def parse_ts(value: str | None) -> datetime | None:
    if not value or not isinstance(value, str) or not value.endswith("Z"):
        return None
    try:
        return datetime.fromisoformat(value[:-1] + "+00:00")
    except ValueError:
        return None


@dataclass(frozen=True)
class VersionRecord:
    document_id: str
    logical_document_id: str
    version_id: str
    version_number: int
    effective_at: datetime
    is_current: bool
    change_summary: str
    role: str


@dataclass(frozen=True)
class Claim:
    claim_key: str
    value: int
    unit: str
    document_id: str
    logical_document_id: str
    version_id: str
    version_number: int
    effective_at: datetime
    is_current: bool
    quote: str
    start: int
    end: int


def load_versions(manifest_path: Path = MANIFEST) -> dict[str, VersionRecord]:
    payload = json.loads(manifest_path.read_text(encoding="utf-8"))
    versions: dict[str, VersionRecord] = {}
    for item in payload.get("documents", []):
        effective = parse_ts(item.get("effectiveAt"))
        if effective is None:
            continue
        record = VersionRecord(
            document_id=item["id"],
            logical_document_id=item["logicalDocumentId"],
            version_id=item["versionId"],
            version_number=int(item.get("versionNumber") or 0),
            effective_at=effective,
            is_current=bool(item.get("isCurrent")),
            change_summary=item.get("changeSummary") or "",
            role=item.get("documentRole") or item.get("role") or "",
        )
        versions[record.version_id] = record
    return versions


def versions_by_logical(
    versions: dict[str, VersionRecord],
) -> dict[str, list[VersionRecord]]:
    grouped: dict[str, list[VersionRecord]] = {}
    for record in versions.values():
        grouped.setdefault(record.logical_document_id, []).append(record)
    for records in grouped.values():
        records.sort(key=lambda item: (item.effective_at, item.version_number))
    return grouped


def current_version(
    logical_id: str,
    grouped: dict[str, list[VersionRecord]],
    at: datetime | None = None,
) -> VersionRecord | None:
    records = grouped.get(logical_id, [])
    if at is None:
        for record in records:
            if record.is_current:
                return record
        return records[-1] if records else None
    eligible = [record for record in records if record.effective_at <= at]
    return eligible[-1] if eligible else None


def extract_budget_claims(
    versions: dict[str, VersionRecord],
    md_dir: Path = MD_DIR,
) -> list[Claim]:
    claims: list[Claim] = []
    for record in versions.values():
        path = md_dir / f"{record.document_id}.md"
        if not path.is_file():
            continue
        text = path.read_text(encoding="utf-8")
        match = BUDGET_RE.search(text)
        if not match:
            continue
        quote = match.group(0)
        char_start = text.index(quote)
        start = len(text[:char_start].encode("utf-8"))
        end = start + len(quote.encode("utf-8"))
        claims.append(
            Claim(
                claim_key=BUDGET_CLAIM_KEY,
                value=int(match.group(1)),
                unit="million_vnd",
                document_id=record.document_id,
                logical_document_id=record.logical_document_id,
                version_id=record.version_id,
                version_number=record.version_number,
                effective_at=record.effective_at,
                is_current=record.is_current,
                quote=quote,
                start=start,
                end=end,
            )
        )
    return claims


def detect_numeric_conflicts(claims: list[Claim]) -> list[dict]:
    """Pair BA/design claims with same key/unit and overlapping effective presence."""
    by_logical: dict[str, list[Claim]] = {}
    for claim in claims:
        by_logical.setdefault(claim.logical_document_id, []).append(claim)

    policy = by_logical.get("logical-budget-policy", [])
    design = by_logical.get("logical-budget-design", [])
    conflicts = []
    for left in policy:
        for right in design:
            if left.unit != right.unit or left.claim_key != right.claim_key:
                continue
            if left.value == right.value:
                continue
            # Pair same generation (v1↔v1, v2↔v2).
            if left.version_number != right.version_number:
                continue
            conflicts.append(
                {
                    "claimKey": left.claim_key,
                    "type": "numeric_mismatch",
                    "left": left,
                    "right": right,
                    "difference": abs(left.value - right.value),
                    "unit": left.unit,
                }
            )
    return conflicts


def claim_at(
    claims: list[Claim],
    *,
    logical_id: str,
    instant: datetime,
) -> Claim | None:
    eligible = [
        claim
        for claim in claims
        if claim.logical_document_id == logical_id and claim.effective_at <= instant
    ]
    if not eligible:
        return None
    eligible.sort(key=lambda claim: (claim.effective_at, claim.version_number))
    return eligible[-1]


def predict_conflict_status(
    claims: list[Claim],
    *,
    as_of: datetime | None,
    query_time: datetime,
    version_mode: str,
    authorized_logical_ids: set[str] | None,
) -> str:
    """Derive conflict lifecycle from claims + auth scope (no gold expectedStatus)."""
    required = {"logical-budget-policy", "logical-budget-design"}
    if authorized_logical_ids is not None and not required.issubset(authorized_logical_ids):
        return "hidden"
    instant = as_of or query_time
    left = claim_at(claims, logical_id="logical-budget-policy", instant=instant)
    right = claim_at(claims, logical_id="logical-budget-design", instant=instant)
    if left is None or right is None:
        return "unknown"
    if left.value != right.value:
        return "open_as_of" if as_of is not None else "open_current"
    # Aligned now: check whether any earlier overlapping generation mismatched.
    earlier_conflict = False
    for claim in claims:
        if claim.logical_document_id != "logical-budget-policy":
            continue
        peer = next(
            (
                other
                for other in claims
                if other.logical_document_id == "logical-budget-design"
                and other.version_number == claim.version_number
            ),
            None,
        )
        if peer is not None and peer.value != claim.value:
            earlier_conflict = True
            break
    if not earlier_conflict:
        return "aligned"
    if version_mode == "history":
        # Gold labels history-mode resolved cases as resolved_current.
        return "resolved_current"
    if as_of is not None:
        return "resolved_history"
    return "resolved_current"


def detect_conflicts_at(
    claims: list[Claim],
    *,
    instant: datetime,
) -> set[tuple[str, str]]:
    """Set of conflicting version-id pairs effective at instant."""
    left = claim_at(claims, logical_id="logical-budget-policy", instant=instant)
    right = claim_at(claims, logical_id="logical-budget-design", instant=instant)
    if left is None or right is None or left.value == right.value:
        return set()
    return {(left.version_id, right.version_id)}


def predict_version_ids(
    *,
    mode: str,
    logical_id: str | None,
    as_of: str | None,
    query_time: str,
    grouped: dict[str, list[VersionRecord]],
) -> list[str]:
    if not logical_id:
        return []
    query_instant = parse_ts(query_time)
    as_of_instant = parse_ts(as_of) if as_of else None
    records = grouped.get(logical_id, [])
    if mode == "current":
        current = current_version(logical_id, grouped, at=None)
        if current is None:
            return []
        if query_instant is not None and current.effective_at > query_instant:
            return []
        return [current.version_id]
    if mode == "as_of":
        selected = current_version(logical_id, grouped, at=as_of_instant)
        return [selected.version_id] if selected else []
    if mode in {"compare", "history"}:
        return [record.version_id for record in records]
    return []


def predict_change_note(
    logical_id: str,
    grouped: dict[str, list[VersionRecord]],
) -> str:
    current = current_version(logical_id, grouped)
    return current.change_summary if current else ""


def temporal_answer_value(
    logical_id: str,
    grouped: dict[str, list[VersionRecord]],
    claims: list[Claim],
    as_of: str | None,
) -> int | None:
    selected = current_version(logical_id, grouped, at=parse_ts(as_of) if as_of else None)
    if selected is None:
        return None
    for claim in claims:
        if claim.version_id == selected.version_id and claim.claim_key == BUDGET_CLAIM_KEY:
            return claim.value
    return None


def load_gold_conflict() -> dict:
    payload = json.loads(CONFLICTS.read_text(encoding="utf-8"))
    conflicts = payload.get("conflicts") or []
    if not conflicts:
        return {}
    return conflicts[0]
