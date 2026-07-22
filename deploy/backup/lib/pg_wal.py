#!/usr/bin/env python3
"""PostgreSQL 18 backup_label / backup_manifest WAL coverage (fail-closed)."""

from __future__ import annotations

import json
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable


class PgWalError(ValueError):
    """Fail-closed WAL/backup metadata error."""


LSN_RE = re.compile(r"^[0-9A-F]+/[0-9A-F]+$")
WAL_SEG_RE = re.compile(r"^[0-9A-F]{24}$")
HISTORY_RE = re.compile(r"^[0-9A-F]{8}\.history$")


@dataclass(frozen=True)
class BackupLabel:
    start_lsn: str
    stop_lsn: str
    checkpoint_lsn: str
    timeline_id: int
    start_wal_file: str | None


@dataclass(frozen=True)
class WalRange:
    timeline_id: int
    start_lsn: str
    end_lsn: str


def parse_lsn(value: str) -> int:
    if not LSN_RE.fullmatch(value):
        raise PgWalError(f"invalid LSN: {value!r}")
    hi, lo = value.split("/")
    return (int(hi, 16) << 32) | int(lo, 16)


def format_lsn(value: int) -> str:
    return f"{value >> 32:X}/{value & 0xFFFFFFFF:X}"


def parse_backup_label(text: str) -> BackupLabel:
    """Parse backup_label with no silent fallbacks."""
    start_m = re.search(r"^START WAL LOCATION:\s*([0-9A-F]+/[0-9A-F]+)", text, re.M)
    stop_m = re.search(r"^STOP WAL LOCATION:\s*([0-9A-F]+/[0-9A-F]+)", text, re.M)
    ckpt_m = re.search(r"^CHECKPOINT LOCATION:\s*([0-9A-F]+/[0-9A-F]+)", text, re.M)
    tl_m = re.search(r"^START TIMELINE:\s*(\d+)\s*$", text, re.M)
    file_m = re.search(r"^START WAL LOCATION:\s*[0-9A-F]+/[0-9A-F]+\s*\(file\s+([0-9A-F]{24})\)", text, re.M)
    if not start_m:
        raise PgWalError("backup_label missing START WAL LOCATION")
    if not stop_m:
        raise PgWalError("backup_label missing STOP WAL LOCATION (no fallback)")
    if not ckpt_m:
        raise PgWalError("backup_label missing CHECKPOINT LOCATION")
    if not tl_m:
        raise PgWalError("backup_label missing START TIMELINE")
    start_lsn = start_m.group(1)
    stop_lsn = stop_m.group(1)
    if parse_lsn(stop_lsn) < parse_lsn(start_lsn):
        raise PgWalError("backup_label STOP WAL LOCATION precedes START")
    return BackupLabel(
        start_lsn=start_lsn,
        stop_lsn=stop_lsn,
        checkpoint_lsn=ckpt_m.group(1),
        timeline_id=int(tl_m.group(1)),
        start_wal_file=file_m.group(1) if file_m else None,
    )


def parse_backup_manifest(text: str) -> list[WalRange]:
    """Parse PostgreSQL 18 backup_manifest WAL-Ranges (required)."""
    try:
        data = json.loads(text)
    except json.JSONDecodeError as error:
        raise PgWalError(f"backup_manifest JSON invalid: {error}") from error
    if not isinstance(data, dict):
        raise PgWalError("backup_manifest root must be object")
    version = data.get("PostgreSQL-Backup-Manifest-Version")
    if version not in (1, 2):
        raise PgWalError(f"unsupported backup_manifest version: {version!r}")
    ranges = data.get("WAL-Ranges")
    if not isinstance(ranges, list) or not ranges:
        raise PgWalError("backup_manifest missing WAL-Ranges")
    out: list[WalRange] = []
    for idx, item in enumerate(ranges):
        if not isinstance(item, dict):
            raise PgWalError(f"WAL-Ranges[{idx}] must be object")
        timeline = item.get("Timeline")
        start = item.get("Start-LSN")
        end = item.get("End-LSN")
        if not isinstance(timeline, int) or timeline < 1:
            raise PgWalError(f"WAL-Ranges[{idx}].Timeline invalid")
        if not isinstance(start, str) or not isinstance(end, str):
            raise PgWalError(f"WAL-Ranges[{idx}] Start/End-LSN missing")
        if parse_lsn(end) < parse_lsn(start):
            raise PgWalError(f"WAL-Ranges[{idx}] End-LSN precedes Start-LSN")
        out.append(WalRange(timeline_id=timeline, start_lsn=start, end_lsn=end))
    return out


def wal_segment_name(timeline_id: int, lsn: int, *, wal_seg_size: int = 16 * 1024 * 1024) -> str:
    if timeline_id < 1 or wal_seg_size <= 0:
        raise PgWalError("invalid timeline/segsize")
    logid = lsn >> 32
    seg = (lsn & 0xFFFFFFFF) // wal_seg_size
    return f"{timeline_id:08X}{logid:08X}{seg:08X}"


def segments_for_range(range_: WalRange, *, wal_seg_size: int = 16 * 1024 * 1024) -> list[str]:
    start = parse_lsn(range_.start_lsn)
    end = parse_lsn(range_.end_lsn)
    # Inclusive coverage of every segment touched through end LSN.
    names: list[str] = []
    cur = start - (start % wal_seg_size)
    while cur <= end:
        names.append(wal_segment_name(range_.timeline_id, cur, wal_seg_size=wal_seg_size))
        cur += wal_seg_size
    if not names:
        raise PgWalError("WAL range produced no segments")
    return names


def classify_wal_entry(name: str) -> str:
    base = Path(name).name
    if WAL_SEG_RE.fullmatch(base):
        return "segment"
    if HISTORY_RE.fullmatch(base):
        return "history"
    return "junk"


def validate_wal_coverage(
    *,
    label: BackupLabel,
    ranges: list[WalRange],
    wal_names: Iterable[str],
    target_lsn: str | None = None,
) -> dict[str, Any]:
    """Require exact segment/history coverage through target LSN; reject junk."""
    target = target_lsn or label.stop_lsn
    target_int = parse_lsn(target)
    if target_int < parse_lsn(label.start_lsn) or target_int > parse_lsn(label.stop_lsn):
        raise PgWalError("target LSN outside backup_label start/stop")
    matching = [r for r in ranges if r.timeline_id == label.timeline_id]
    if not matching:
        raise PgWalError("no WAL-Ranges for backup_label timeline")
    covering = [
        r
        for r in matching
        if parse_lsn(r.start_lsn) <= parse_lsn(label.start_lsn)
        and parse_lsn(r.end_lsn) >= target_int
    ]
    if not covering:
        raise PgWalError("WAL-Ranges do not cover target LSN on timeline")

    present = [n for n in wal_names if n and n not in {".", "./"}]
    junk = [n for n in present if classify_wal_entry(n) == "junk"]
    if junk:
        raise PgWalError(f"junk/non-WAL entries in WAL package: {junk[:5]}")
    segments = {Path(n).name for n in present if classify_wal_entry(n) == "segment"}
    history = {Path(n).name for n in present if classify_wal_entry(n) == "history"}
    required: set[str] = set()
    for range_ in covering:
        required.update(segments_for_range(range_))
    # Trim required segments to those intersecting [start, target].
    required = {
        name
        for name in required
        if _segment_touches(name, parse_lsn(label.start_lsn), target_int)
    }
    missing = sorted(required - segments)
    if missing:
        raise PgWalError(f"missing WAL segments through target LSN: {missing[:8]}")
    expected_history = f"{label.timeline_id:08X}.history"
    # history file optional for timeline 1 on greenfield, required if present elsewhere.
    if label.timeline_id > 1 and expected_history not in history:
        raise PgWalError(f"missing timeline history file {expected_history}")
    return {
        "timelineId": label.timeline_id,
        "startWalLsn": label.start_lsn,
        "stopWalLsn": label.stop_lsn,
        "targetLsn": target,
        "requiredSegments": sorted(required),
        "presentSegments": sorted(segments),
        "historyFiles": sorted(history),
    }


def _segment_touches(name: str, start: int, end: int, *, wal_seg_size: int = 16 * 1024 * 1024) -> bool:
    if not WAL_SEG_RE.fullmatch(name):
        return False
    logid = int(name[8:16], 16)
    seg = int(name[16:24], 16)
    seg_start = (logid << 32) | (seg * wal_seg_size)
    seg_end = seg_start + wal_seg_size - 1
    return seg_end >= start and seg_start <= end
