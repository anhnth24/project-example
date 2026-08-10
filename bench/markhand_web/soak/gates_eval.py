"""Threshold loading and numeric gate evaluation for P1B-O05."""

from __future__ import annotations

import json
import hashlib
import math
from pathlib import Path
from typing import Any


GATE_QUERY_P95 = "G0-SLO-QUERY-P95"
GATE_QUERY_P99 = "G0-SLO-QUERY-P99"
# The soak qualifies the poc-compose environment, whose per-service CPU caps and
# mock embedding cannot represent Profile B. The peak-tier production gate
# (G0-CAP-INGEST-THROUGHPUT, on-prem-reference) is measured separately.
GATE_INGEST = "G0-CAP-INGEST-THROUGHPUT-POC"

OFFICIAL_DURATION_SECONDS = 1800
CANONICAL_PROFILE_SHA256 = "ee99f59424bf967d8e52572687ad38a9efe0976e693e56cb54233bb64330192e"
CANONICAL_GATES_SHA256 = "a8d5836c0ab3c239a6aa4b9b7813404d9cdb4f1b7fa9aacb18ac8bc17508aaa4"
CANONICAL_THRESHOLDS = {
    "queryP95Ms": 500.0,
    "queryP99Ms": 1000.0,
    "ingestDocsPerHour": 300.0,
    "maxRssGrowthMb": 256.0,
    "maxTempGrowthMb": 512.0,
    "maxQueueDepth": 100,
    "maxDbConnections": 40,
}
# POC qualification: zero request errors outside the exact injection window.
ALLOWED_ERRORS_OUTSIDE_INJECTION = 0
COMPLETENESS_RATIO = 0.95
MIN_RESOURCE_COVERAGE_RATIO = 0.90
MIN_QUERY_SUCCESS_SAMPLES = 100


def _sha256(path: Path) -> str | None:
    if not path.is_file():
        return None
    # Git may materialize YAML as CRLF on Windows and LF in Linux/CI. Bind the
    # semantic text artifact, not the checkout-specific newline representation.
    canonical = path.read_bytes().replace(b"\r\n", b"\n").replace(b"\r", b"\n")
    return hashlib.sha256(canonical).hexdigest()


def _load_gates_doc(path: Path) -> dict[str, Any]:
    text = path.read_text(encoding="utf-8")
    return json.loads(text)


def _threshold_value(gates_doc: dict[str, Any], gate_id: str) -> float | None:
    for gate in gates_doc.get("gates") or []:
        if not isinstance(gate, dict):
            continue
        if gate.get("id") != gate_id:
            continue
        thr = gate.get("threshold") or {}
        if isinstance(thr, dict) and "value" in thr:
            if isinstance(thr["value"], bool) or not isinstance(thr["value"], (int, float)):
                raise RuntimeError(f"threshold {gate_id} must be finite nonnegative numeric")
            if not math.isfinite(float(thr["value"])) or float(thr["value"]) < 0.0:
                raise RuntimeError(f"threshold {gate_id} must be finite nonnegative numeric")
            return float(thr["value"])
    return None


def load_thresholds(profile: dict[str, Any], gates_path: Path | str) -> dict[str, Any]:
    """Combine profile bounds + gates.yaml + SLA targets into binding thresholds."""
    gates_path = Path(gates_path)
    gates_doc = _load_gates_doc(gates_path)
    bounds = profile.get("bounds") or {}
    p95 = _threshold_value(gates_doc, GATE_QUERY_P95)
    p99 = _threshold_value(gates_doc, GATE_QUERY_P99)
    ingest = _threshold_value(gates_doc, GATE_INGEST)
    if p95 is None or p99 is None or ingest is None:
        raise RuntimeError("binding SLO/CAP thresholds missing from gates.yaml")
    profile_path = Path(str(profile.get("sourcePath") or ""))
    profile_sha = _sha256(profile_path)
    gates_sha = _sha256(gates_path)
    duration = int(profile.get("durationSeconds") or 0)
    values = {
        "queryP95Ms": p95,
        "queryP99Ms": p99,
        "ingestDocsPerHour": ingest,
        "maxRssGrowthMb": float(bounds.get("maxRssGrowthMb", 256)),
        "maxTempGrowthMb": float(bounds.get("maxTempGrowthMb", 512)),
        "maxQueueDepth": int(bounds.get("maxQueueDepth", 100)),
        "maxDbConnections": int(bounds.get("maxDbConnections", 40)),
    }
    canonical_values = all(values[k] == v for k, v in CANONICAL_THRESHOLDS.items())
    return {
        **values,
        "ingestGateBinding": True,
        "officialDurationSeconds": OFFICIAL_DURATION_SECONDS,
        "profileDurationSeconds": duration,
        "profileSha256": profile_sha,
        "gatesSha256": gates_sha,
        "canonicalProfileSha256": CANONICAL_PROFILE_SHA256,
        "canonicalGatesSha256": CANONICAL_GATES_SHA256,
        "canonicalThresholdValues": dict(CANONICAL_THRESHOLDS),
        "canonicalBindingPass": bool(
            duration == OFFICIAL_DURATION_SECONDS
            and profile_sha == CANONICAL_PROFILE_SHA256
            and gates_sha == CANONICAL_GATES_SHA256
            and canonical_values
        ),
        "allowedErrorsOutsideInjection": ALLOWED_ERRORS_OUTSIDE_INJECTION,
        "completenessRatio": COMPLETENESS_RATIO,
        "minResourceCoverageRatio": MIN_RESOURCE_COVERAGE_RATIO,
        "minQuerySuccessSamples": MIN_QUERY_SUCCESS_SAMPLES,
        "rpoMinutes": 15,
        "queryReadyRtoMinutes": 60,
        "fullVectorRtoMinutes": 240,
    }


def _cmp_le(actual: float | None, limit: float) -> str:
    if actual is None:
        return "unknown"
    if isinstance(actual, bool) or not isinstance(actual, (int, float)):
        return "fail"
    if not math.isfinite(float(actual)) or float(actual) < 0.0:
        return "fail"
    return "pass" if actual <= limit else "fail"


def _cmp_ge(actual: float | None, limit: float) -> str:
    if actual is None:
        return "unknown"
    if isinstance(actual, bool) or not isinstance(actual, (int, float)):
        return "fail"
    if not math.isfinite(float(actual)) or float(actual) < 0.0:
        return "fail"
    return "pass" if actual >= limit else "fail"


def evaluate_numeric_gates(
    metrics: dict[str, Any],
    thresholds: dict[str, Any],
) -> dict[str, str]:
    """Evaluate measured metrics against exact binding thresholds.

    Returns gate name → pass|fail|unknown. Never invents pass for missing numbers.
    Zero successful query samples ⇒ query latency gates unknown/fail (not pass).
    """
    modes_ready = metrics.get("queryModesReady")
    query_samples = metrics.get("querySuccessSamples")
    if query_samples is None:
        query_samples = 0
    min_query_samples = int(thresholds.get("minQuerySuccessSamples", 1))
    if not modes_ready or int(query_samples) < min_query_samples:
        query_p95 = "fail" if metrics.get("measured") else "unknown"
        query_p99 = query_p95
        # When measured but zero samples: fail. When not measured: unknown.
        if metrics.get("measured") is True:
            query_p95 = "fail"
            query_p99 = "fail"
        else:
            query_p95 = "unknown"
            query_p99 = "unknown"
    else:
        query_p95 = _cmp_le(metrics.get("queryP95Ms"), float(thresholds["queryP95Ms"]))
        query_p99 = _cmp_le(metrics.get("queryP99Ms"), float(thresholds["queryP99Ms"]))

    canonical = "pass" if thresholds.get("canonicalBindingPass") else "fail"

    completeness = metrics.get("completenessPassed")
    if completeness is False:
        # Completeness shortfall fails throughput/latency qualification.
        if query_p95 == "pass":
            query_p95 = "fail"
        if query_p99 == "pass":
            query_p99 = "fail"

    if thresholds.get("ingestGateBinding"):
        if metrics.get("ingestOk") in (None, 0) and metrics.get("measured") is True:
            ingest = "fail"
        else:
            ingest = _cmp_ge(
                metrics.get("ingestDocsPerHour"), float(thresholds["ingestDocsPerHour"])
            )
        if completeness is False and ingest == "pass":
            ingest = "fail"
    else:
        ingest = "unknown"

    rss = _cmp_le(metrics.get("rssGrowthMb"), float(thresholds["maxRssGrowthMb"]))
    temp = _cmp_le(metrics.get("tempGrowthMb"), float(thresholds["maxTempGrowthMb"]))
    queue = _cmp_le(
        metrics.get("queueDepthMax") if metrics.get("queueDepthMax") is not None else None,
        float(thresholds["maxQueueDepth"]),
    )
    dbconn = _cmp_le(
        metrics.get("dbConnectionsMax")
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

    # Request error gate (outside injection window).
    allowed = int(thresholds.get("allowedErrorsOutsideInjection", 0))
    err_out = metrics.get("requestErrorsOutsideInjection")
    if err_out is None:
        errors_gate = "unknown"
    elif int(err_out) > allowed:
        errors_gate = "fail"
    else:
        errors_gate = "pass"

    if completeness is True:
        completeness_gate = "pass"
    elif completeness is False:
        completeness_gate = "fail"
    else:
        completeness_gate = "unknown"

    drain = metrics.get("workloadDrainPassed")
    if drain is True:
        drain_gate = "pass"
    elif drain is False:
        drain_gate = "fail"
    else:
        drain_gate = "unknown"

    reconcile = metrics.get("reconcilePassed")
    if reconcile is True:
        reconcile_gate = "pass"
    elif reconcile is False:
        reconcile_gate = "fail"
    else:
        reconcile_gate = "unknown"

    resource = metrics.get("resourceCoveragePassed")
    if resource is True:
        resource_gate = "pass"
    elif resource is False:
        resource_gate = "fail"
    else:
        resource_gate = "unknown"

    return {
        "canonicalBinding": canonical,
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
        "requestErrors": errors_gate,
        "completeness": completeness_gate,
        "workloadDrain": drain_gate,
        "reconcile": reconcile_gate,
        "resourceCoverage": resource_gate,
    }
