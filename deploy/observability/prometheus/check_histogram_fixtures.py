#!/usr/bin/env python3
"""Invariant checker for physically valid Prometheus histogram fixtures.

Ensures each timestamp in markhand-rules-test.yml histogram series has:
- cumulative le buckets (value[le_i] <= value[le_{i+1}])
- non-decreasing counts over time per series
- matching +Inf presence for each label set
"""

from __future__ import annotations

import re
import sys
from collections import defaultdict
from pathlib import Path


def expand_values(spec: str) -> list[float]:
    """Expand promtool values syntax enough for our fixtures."""
    parts: list[float] = []
    for token in spec.split():
        m = re.fullmatch(r"([0-9.eE+-]+)(?:\+([0-9.eE+-]+)x([0-9]+))?", token)
        if not m:
            # plain '0x25' form
            m2 = re.fullmatch(r"([0-9.eE+-]+)x([0-9]+)", token)
            if not m2:
                raise ValueError(f"unsupported values token: {token!r}")
            base = float(m2.group(1))
            n = int(m2.group(2))
            parts.extend([base] * (n + 1))
            continue
        start = float(m.group(1))
        if m.group(2) is None:
            parts.append(start)
            continue
        step = float(m.group(2))
        n = int(m.group(3))
        for i in range(n + 1):
            parts.append(start + step * i)
    return parts


def parse_histogram_series(text: str) -> dict[tuple[str, str], dict[str, list[float]]]:
    """Map (metric, labels_without_le) -> {le: values}."""
    out: dict[tuple[str, str], dict[str, list[float]]] = defaultdict(dict)
    for series, values in re.findall(
        r"- series:\s*'([^']+)'\s*\n\s*values:\s*\"([^\"]+)\"", text
    ):
        if "_bucket" not in series or "le=" not in series:
            continue
        m = re.match(r"([a-z0-9_:]+)\{(.+)\}", series)
        if not m:
            continue
        metric, labels = m.group(1), m.group(2)
        le_m = re.search(r'le="([^"]+)"', labels)
        if not le_m:
            continue
        le = le_m.group(1)
        base_labels = re.sub(r',?le="[^"]+"', "", labels).strip(", ")
        out[(metric, base_labels)][le] = expand_values(values)
    return out


def le_sort_key(le: str) -> float:
    if le == "+Inf":
        return float("inf")
    return float(le)


def check(path: Path) -> list[str]:
    text = path.read_text(encoding="utf-8")
    errors: list[str] = []
    series_map = parse_histogram_series(text)
    if not series_map:
        errors.append("no histogram bucket fixtures found")
        return errors
    for (metric, labels), buckets in series_map.items():
        if "+Inf" not in buckets:
            errors.append(f"{metric}{{{labels}}}: missing +Inf bucket")
        lengths = {le: len(vals) for le, vals in buckets.items()}
        if len(set(lengths.values())) != 1:
            errors.append(f"{metric}{{{labels}}}: mismatched series lengths {lengths}")
            continue
        n = next(iter(lengths.values()))
        ordered = sorted(buckets.keys(), key=le_sort_key)
        for t in range(n):
            prev = -1.0
            for le in ordered:
                v = buckets[le][t]
                if v < prev - 1e-9:
                    errors.append(
                        f"{metric}{{{labels}}}: non-cumulative at t={t}: le={le} value={v} < prev={prev}"
                    )
                    break
                prev = v
        for le, vals in buckets.items():
            for i in range(1, len(vals)):
                if vals[i] + 1e-9 < vals[i - 1]:
                    errors.append(
                        f"{metric}{{{labels}}} le={le}: decreased at t={i}: {vals[i-1]} -> {vals[i]}"
                    )
                    break
    return errors


def main() -> int:
    path = Path(sys.argv[1] if len(sys.argv) > 1 else "markhand-rules-test.yml")
    errors = check(path)
    if errors:
        print("HISTOGRAM_FIXTURE_FAIL")
        for e in errors:
            print(f"  - {e}")
        return 1
    print(f"histogram fixtures OK ({path})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
