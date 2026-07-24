"""Threshold loading and numeric gate evaluation for P1B-O05."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any


GATE_QUERY_P95 = "G0-SLO-QUERY-P95"
GATE_QUERY_P99 = "G0-SLO-QUERY-P99"
GATE_INGEST = "G0-CAP-INGEST-THROUGHPUT"

OFFICIAL_DURATION_SECONDS = 1800


def _load_gates_doc(path: Path) -> dict[str, Any]:
    text = path.read_text(encoding="utf-8")
    # gates.yaml is JSON-shaped in this repository.
    return json.loads(text)


def _threshold_value(gates_doc: dict[str, Any], gate_id: str) -> float | None:
    for gate in gates_doc.get("gates") or []:
        if not isinstance(gate, dict):
            continue
        if gate.get("id") != gate_id:
            continue
        thr = gate.get("threshold") or {}
        if isinstance(thr, dict) and "value" in thr:
            return float(thr["value"])
    return None


def load_thresholds(profile: dict[str, Any], gates_path: Path | str) -> dict[str, Any]:
    """Combine profile bounds + gates.yaml + SLA targets into binding thresholds."""
    gates_doc = _load_gates_doc(Path(gates_path))
    bounds = profile.get("bounds") or {}
    p95 = _threshold_value(gates_doc, GATE_QUERY_P95)
    p99 = _threshold_value(gates_doc, GATE_QUERY_P99)
    ingest = _threshold_value(gates_doc, GATE_INGEST)
    if p95 is None or p99 is None or ingest is None:
        raise RuntimeError("binding SLO/CAP thresholds missing from gates.yaml")
    return {
        "queryP95Ms": p95,
        "queryP99Ms": p99,
        "ingestDocsPerHour": ingest,
        "ingestGateBinding": True,
        "maxRssGrowthMb": float(bounds.get("maxRssGrowthMb", 256)),
        "maxTempGrowthMb": float(bounds.get("maxTempGrowthMb", 512)),
        "maxQueueDepth": int(bounds.get("maxQueueDepth", 100)),
        "maxDbConnections": int(bounds.get("maxDbConnections", 40)),
        "officialDurationSeconds": int(
            profile.get("durationSeconds") or OFFICIAL_DURATION_SECONDS
        ),
        "rpoMinutes": 15,
        "queryReadyRtoMinutes": 60,
        "fullVectorRtoMinutes": 240,
    }


def _cmp_le(actual: float | None, limit: float) -> str:
    if actual is None:
        return "unknown"
    return "pass" if actual <= limit else "fail"


def _cmp_ge(actual: float | None, limit: float) -> str:
    if actual is None:
        return "unknown"
    return "pass" if actual >= limit else "fail"


def evaluate_numeric_gates(
    metrics: dict[str, Any],
    thresholds: dict[str, Any],
) -> dict[str, str]:
    """Evaluate measured metrics against exact binding thresholds.

    Returns gate name → pass|fail|unknown. Never invents pass for missing numbers.
    """
    query_p95 = _cmp_le(metrics.get("queryP95Ms"), float(thresholds["queryP95Ms"]))
    query_p99 = _cmp_le(metrics.get("queryP99Ms"), float(thresholds["queryP99Ms"]))

    if thresholds.get("ingestGateBinding"):
        ingest = _cmp_ge(
            metrics.get("ingestDocsPerHour"), float(thresholds["ingestDocsPerHour"])
        )
    else:
        ingest = "unknown"

    rss = _cmp_le(metrics.get("rssGrowthMb"), float(thresholds["maxRssGrowthMb"]))
    temp = _cmp_le(metrics.get("tempGrowthMb"), float(thresholds["maxTempGrowthMb"]))
    queue = _cmp_le(
        float(metrics["queueDepthMax"]) if metrics.get("queueDepthMax") is not None else None,
        float(thresholds["maxQueueDepth"]),
    )
    dbconn = _cmp_le(
        float(metrics["dbConnectionsMax"])
        if metrics.get("dbConnectionsMax") is not None
        else None,
        float(thresholds["maxDbConnections"]),
    )

    growth_parts = [rss, temp, queue, dbconn]
    if any(p == "fail" for p in growth_parts):
        unbounded = "fail"
    elif any(p == "unknown" for p in growth_parts):
        unbounded = "unknown"
    else:
        unbounded = "pass"

    worker = metrics.get("workerRecoveryPass")
    dep = metrics.get("dependencyRecoveryPass")
    if worker is True and dep is True:
        recovery = "pass"
    elif worker is False or dep is False:
        recovery = "fail"
    else:
        recovery = "unknown"

    post = metrics.get("postRestoreRetrievalPass")
    if post is True:
        post_restore = "pass"
    elif post is False:
        post_restore = "fail"
    else:
        post_restore = "unknown"

    return {
        "queryP95": query_p95,
        "queryP99": query_p99,
        "ingestThroughput": ingest,
        "rssGrowth": rss,
        "tempGrowth": temp,
        "queueDepth": queue,
        "dbConnections": dbconn,
        "unboundedGrowth": unbounded,
        "recovery": recovery,
        "postRestoreRetrieval": post_restore,
    }
