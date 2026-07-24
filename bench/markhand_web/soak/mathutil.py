"""Percentile math and rate scheduling for P1B-O05."""

from __future__ import annotations

import random
from typing import Sequence


def percentile(samples: Sequence[float], pct: float) -> float | None:
    """Nearest-rank linear interpolation percentile (inclusive).

    ``pct`` is in 0..100. Empty input returns None.
    """
    if not samples:
        return None
    ordered = sorted(float(x) for x in samples)
    if len(ordered) == 1:
        return ordered[0]
    if pct <= 0:
        return ordered[0]
    if pct >= 100:
        return ordered[-1]
    # Excel-style PERCENTILE.INC: rank = pct/100 * (n-1)
    rank = (pct / 100.0) * (len(ordered) - 1)
    lo = int(rank)
    hi = min(lo + 1, len(ordered) - 1)
    frac = rank - lo
    return ordered[lo] * (1.0 - frac) + ordered[hi] * frac


def schedule_event_times(
    *,
    rps: float,
    duration_seconds: float,
    seed: int | None = None,
) -> list[float]:
    """Deterministic-ish event schedule at approximately ``rps`` for ``duration_seconds``.

    Uses fixed-interval scheduling with tiny seeded jitter so concurrent actors
    do not perfectly align. Returns offsets in [0, duration).
    """
    if rps <= 0 or duration_seconds <= 0:
        return []
    interval = 1.0 / rps
    rng = random.Random(seed)
    times: list[float] = []
    t = 0.0
    # Start with a half-interval offset so the first event is not always t=0.
    t = interval * 0.5
    while t < duration_seconds:
        jitter = (rng.random() - 0.5) * interval * 0.1
        stamped = t + jitter
        if 0.0 <= stamped < duration_seconds:
            times.append(stamped)
        t += interval
    return sorted(times)
