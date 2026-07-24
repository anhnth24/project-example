"""Workload profile loader for phase1b-mixed.yaml (stdlib-only)."""

from __future__ import annotations

import re
from pathlib import Path
from typing import Any


class ProfileError(RuntimeError):
    """Invalid or incomplete workload profile."""


_FORMATS_RE = re.compile(r"formats:\s*\[([^\]]+)\]")
_MODES_RE = re.compile(r"modes:\s*\[([^\]]+)\]")


def _list_items(match: re.Match[str] | None) -> list[str]:
    if not match:
        return []
    return [part.strip().lower() for part in match.group(1).split(",") if part.strip()]


def _scalar(text: str, key: str, cast=float):
    match = re.search(rf"^[ \t]*{re.escape(key)}:\s*(.+)$", text, re.MULTILINE)
    if not match:
        return None
    raw = match.group(1).strip().strip("\"'")
    try:
        return cast(raw)
    except (TypeError, ValueError) as exc:
        raise ProfileError(f"invalid {key}: {raw!r}") from exc


def load_workload_profile(path: Path | str) -> dict[str, Any]:
    """Parse the Phase 1B mixed-load YAML without requiring PyYAML."""
    path = Path(path)
    text = path.read_text(encoding="utf-8")
    name = _scalar(text, "name", str)
    duration = _scalar(text, "durationSeconds", int)
    if not name or duration is None:
        raise ProfileError(f"name/durationSeconds missing in {path}")

    # Per-actor rps keys are parsed from sections (file has multiple `rps:` keys).
    ingest_block = re.search(
        r"ingest:\s*\n((?:[ \t]+.+\n)+)",
        text,
    )
    query_block = re.search(
        r"query:\s*\n((?:[ \t]+.+\n)+)",
        text,
    )
    delete_block = re.search(
        r"delete:\s*\n((?:[ \t]+.+\n)+)",
        text,
    )
    reconcile_block = re.search(
        r"reconcile:\s*\n((?:[ \t]+.+\n)+)",
        text,
    )
    failure_block = re.search(
        r"failureInjection:\s*\n((?:[ \t]+.+\n)+)",
        text,
    )
    bounds_block = re.search(
        r"bounds:\s*\n((?:[ \t]+.+\n)+)",
        text,
    )

    def block_scalar(block: str | None, key: str, cast=float):
        if not block:
            return None
        return _scalar(block, key, cast)

    formats = _list_items(_FORMATS_RE.search(ingest_block.group(1) if ingest_block else ""))
    modes = _list_items(_MODES_RE.search(query_block.group(1) if query_block else ""))
    if not formats:
        raise ProfileError(f"ingest formats missing in {path}")

    profile = {
        "name": name,
        "durationSeconds": int(duration),
        "actors": {
            "ingest": {
                "rps": float(block_scalar(ingest_block.group(1) if ingest_block else None, "rps") or 0.0),
                "formats": formats,
            },
            "query": {
                "rps": float(block_scalar(query_block.group(1) if query_block else None, "rps") or 0.0),
                "modes": modes or ["current"],
            },
            "delete": {
                "rps": float(block_scalar(delete_block.group(1) if delete_block else None, "rps") or 0.0),
            },
            "reconcile": {
                "intervalSeconds": int(
                    block_scalar(
                        reconcile_block.group(1) if reconcile_block else None,
                        "intervalSeconds",
                        int,
                    )
                    or 300
                ),
            },
        },
        "failureInjection": {
            "killWorkerEverySeconds": int(
                block_scalar(
                    failure_block.group(1) if failure_block else None,
                    "killWorkerEverySeconds",
                    int,
                )
                or 0
            ),
            "dependencyBlipSeconds": int(
                block_scalar(
                    failure_block.group(1) if failure_block else None,
                    "dependencyBlipSeconds",
                    int,
                )
                or 0
            ),
        },
        "bounds": {
            "maxRssGrowthMb": int(
                block_scalar(bounds_block.group(1) if bounds_block else None, "maxRssGrowthMb", int)
                or 256
            ),
            "maxTempGrowthMb": int(
                block_scalar(bounds_block.group(1) if bounds_block else None, "maxTempGrowthMb", int)
                or 512
            ),
            "maxQueueDepth": int(
                block_scalar(bounds_block.group(1) if bounds_block else None, "maxQueueDepth", int)
                or 100
            ),
            "maxDbConnections": int(
                block_scalar(
                    bounds_block.group(1) if bounds_block else None, "maxDbConnections", int
                )
                or 40
            ),
        },
        "sourcePath": str(path),
    }
    return profile
