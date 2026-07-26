"""Gate evaluation + canonical report I/O for the P1B-R06 hanging-dependency soak.

Fail-closed, same house style as `bench/markhand_web/soak/report.py` (P1B-O05):
default is an honest `not_run`; opting in without complete evidence is
`incomplete`/`fail`, never `pass`; a claimed `pass` carries the provenance a
later reviewer needs to re-check it (full git SHA, compose project, image
ids, a raw evidence directory with a sha256 manifest) — see
`crates/server/tests/e2e_release_suite.rs`'s
`assert_committed_o05_pass_is_attested` for the shape this is modeled on.

This module intentionally does not import anything from `bench/markhand_web/
soak/` (the O05 package) except the generic, harness-agnostic `redact`
module, so the two issues' evidence pipelines stay independent.
"""

from __future__ import annotations

import hashlib
import json
import os
import sys
import time
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "bench/markhand_web/soak"))
import redact  # noqa: E402  (generic redaction helper shared with O05)

ISSUE = "P1B-R06"
CANONICAL = "r06-hanging-soak.json"
OUT_DIR = ROOT / "bench/markhand_web/reports/phase-1b-gate"
COMPOSE_POC = ROOT / "deploy/compose.poc.yml"
MIGRATIONS_MANIFEST = ROOT / "crates/server/migrations/manifest.json"

# --- Readiness contract constants, cited from the source they must match ---
# crates/server/src/services/readiness.rs:24-25
OUTER_DEADLINE_SECONDS = 4.0
PER_PROBE_DEADLINE_SECONDS = 2.0
# Operational slack for a live Docker network stack (scheduler jitter, the
# loopback hop from the harness to the published API port, JSON body
# transfer). Not part of the readiness contract itself — a harness-side
# choice, documented so it is easy to tighten later. Mirrors the
# `PER_PROBE_DEADLINE + Duration::from_millis(750)` / `+ Duration::from_secs(1)`
# slack the hermetic Rust tests in readiness.rs already use for the same
# reason (see `assert_hanging_http_probe` / `hanging_router_ready_matrix_...`).
READY_DEADLINE_SLACK_SECONDS = 2.0
READY_BOUND_SECONDS = OUTER_DEADLINE_SECONDS + READY_DEADLINE_SLACK_SECONDS  # 6.0

# `/api/v1/health/live` (routes/health.rs `liveness`) and
# `/api/v1/openapi.yaml` (http.rs `openapi_yaml`, serving
# `api::embedded_openapi_yaml()`) never touch a dependency probe — they must
# stay fast regardless of what readiness is doing.
LIVE_BUDGET_SECONDS = 2.0

DEFAULT_SUSTAIN_SECONDS = 60
# This harness's own bar for what counts as a "sustained" run rather than a
# syntax/wiring smoke test. Not a registered bench/markhand_web/gates.yaml
# gate (no G0- id, no owner/approver) — that registration is a decision for
# whoever qualifies R06, not something this harness invents on their behalf.
MIN_QUALIFYING_SUSTAIN_SECONDS = 60
DEFAULT_POLL_INTERVAL_SECONDS = 2.0
DEFAULT_CONCURRENCY = 8
DEFAULT_RECOVERY_DEADLINE_SECONDS = 30.0
# A batch of N concurrent requests must finish in roughly one bounded-deadline
# wall-clock window, not N-times that (which is what "queued behind the
# hung dependency" looks like).
CONCURRENCY_BOUND_SECONDS = READY_BOUND_SECONDS + 2.0
# A later batch taking much longer than the first is a slow leak (e.g. a
# connection pool losing capacity across the sustain window) even if no
# single batch crosses CONCURRENCY_BOUND_SECONDS outright.
CONCURRENCY_GROWTH_FACTOR = 1.75

# Dependencies crates/server/src/services/readiness.rs actually probes over
# the network, and the exact stable code (`ReadinessProbeError::code()`)
# each one must surface. IndexSignature/ActiveGeneration/ReconcileFence/
# *Credentials are deliberately excluded: IndexSignature is a local digest
# check (no network to hang), and ActiveGeneration/ReconcileFence share the
# same Postgres pool as the Database probe and are ordered strictly after
# it in `check_ready_inner`, so pausing postgres always surfaces as
# `ready_database` first — there is no way to isolate those codes with a
# whole-container pause. That is a property of the probe ordering, not a
# gap this harness is hiding; see the R06 report's `notNetworkIsolable`
# field and the runbook doc for the full explanation.
DEPENDENCY_PROBES: dict[str, dict[str, Any]] = {
    "database": {"services": ("postgres",), "expectedCode": "ready_database"},
    "vector_store": {"services": ("qdrant",), "expectedCode": "ready_vector_store"},
    "object_store": {"services": ("minio",), "expectedCode": "ready_object_store"},
    "embedding": {
        "services": ("mock-embedding", "embedding-cpu"),
        "expectedCode": "ready_embedding",
    },
}


def _is_finite_number(value: Any) -> bool:
    return not isinstance(value, bool) and isinstance(value, (int, float))


def evaluate_ready_samples(samples: list[dict[str, Any]], expected_code: str) -> dict[str, Any]:
    """Pure evaluator: every sample taken while the dependency was paused must
    be a 503 with the expected probe code, observed within READY_BOUND_SECONDS.
    """
    if not samples:
        return {"codeCorrect": "unknown", "bounded": "unknown", "sampleCount": 0}
    code_ok = True
    bounded_ok = True
    for sample in samples:
        elapsed = sample.get("elapsedSeconds")
        if not _is_finite_number(elapsed) or elapsed > READY_BOUND_SECONDS:
            bounded_ok = False
        if sample.get("httpStatus") != 503 or sample.get("probeCode") != expected_code:
            code_ok = False
    return {
        "codeCorrect": "pass" if code_ok else "fail",
        "bounded": "pass" if bounded_ok else "fail",
        "sampleCount": len(samples),
    }


def evaluate_business_samples(samples: list[dict[str, Any]]) -> dict[str, Any]:
    """Pure evaluator for /health/live + /openapi.yaml samples taken during the pause."""
    if not samples:
        return {"ok": "unknown", "bounded": "unknown", "sampleCount": 0}
    ok = True
    bounded_ok = True
    for sample in samples:
        elapsed = sample.get("elapsedSeconds")
        if not _is_finite_number(elapsed) or elapsed > LIVE_BUDGET_SECONDS:
            bounded_ok = False
        if sample.get("httpStatus") != 200:
            ok = False
    return {
        "ok": "pass" if ok else "fail",
        "bounded": "pass" if bounded_ok else "fail",
        "sampleCount": len(samples),
    }


def evaluate_concurrency(batches: list[dict[str, Any]]) -> dict[str, Any]:
    """Pure evaluator: nothing may queue without bound behind a paused dependency.

    Two independent checks: (1) every batch's wall-clock span stays within an
    absolute bound regardless of how many requests were fired concurrently;
    (2) later batches are not growing relative to the first one, which would
    indicate a slow leak that never crosses the absolute bound by itself.
    """
    if not batches:
        return {"bounded": "unknown", "noGrowth": "unknown", "batchCount": 0}
    spans = [b.get("spanSeconds") for b in batches]
    if not all(_is_finite_number(s) for s in spans):
        return {"bounded": "fail", "noGrowth": "fail", "batchCount": len(batches)}
    bounded_ok = all(s <= CONCURRENCY_BOUND_SECONDS for s in spans)
    first = spans[0]
    growth_ok = all(
        s <= max(first * CONCURRENCY_GROWTH_FACTOR, CONCURRENCY_BOUND_SECONDS)
        for s in spans[1:]
    ) if len(spans) > 1 else True
    return {
        "bounded": "pass" if bounded_ok else "fail",
        "noGrowth": "pass" if growth_ok else "fail",
        "batchCount": len(batches),
        "spans": spans,
    }


def evaluate_recovery(recovery: dict[str, Any] | None) -> str:
    if not recovery:
        return "unknown"
    if recovery.get("recoveredWithinDeadline") is True and _is_finite_number(
        recovery.get("recoverySeconds")
    ):
        return "pass"
    return "fail"


def evaluate_restore(restore: dict[str, Any] | None) -> str:
    if not restore:
        return "unknown"
    return "pass" if restore.get("restoredConfirmed") is True else "fail"


def evaluate_dependency(result: dict[str, Any]) -> dict[str, Any]:
    """Combine the pure per-signal evaluators into one dependency's gate set + blockers."""
    expected_code = result["expectedProbeCode"]
    ready_eval = evaluate_ready_samples(result.get("samples", {}).get("ready", []), expected_code)
    live_eval = evaluate_business_samples(result.get("samples", {}).get("live", []))
    openapi_eval = evaluate_business_samples(result.get("samples", {}).get("openapi", []))
    concurrency_eval = evaluate_concurrency(result.get("concurrencyBatches", []))
    restore_gate = evaluate_restore(result.get("restore"))
    recovery_gate = evaluate_recovery(result.get("recovery"))

    gates = {
        "readyCodeCorrect": ready_eval["codeCorrect"],
        "readyBounded": ready_eval["bounded"],
        "liveBounded": live_eval["bounded"],
        "openapiBounded": openapi_eval["bounded"],
        "concurrencyBounded": concurrency_eval["bounded"],
        "concurrencyNoGrowth": concurrency_eval["noGrowth"],
        "restoreConfirmed": restore_gate,
        "recoveryBounded": recovery_gate,
    }
    blockers = [f"gate:{name}:{value}" for name, value in gates.items() if value == "fail"]
    if not result.get("pauseConfirmed"):
        blockers.append("pause_not_confirmed")
    return {
        "gates": gates,
        "blockers": blockers,
        "readyEval": ready_eval,
        "liveEval": live_eval,
        "openapiEval": openapi_eval,
        "concurrencyEval": concurrency_eval,
    }


def evaluate_status(
    *,
    opted_in: bool,
    smoke: bool,
    covers_all_dependencies: bool,
    dependency_blockers: list[str],
    redaction_ok: bool,
) -> tuple[str, list[str]]:
    """Fail-closed status combination, mirrored on `soak/report.py::evaluate_status`."""
    blockers: list[str] = []
    if not opted_in:
        blockers.append("MARKHAND_HANGING_SOAK!=1")
        return "not_run", blockers
    if smoke:
        blockers.append("smoke_non_qualifying_sustain")
    if not covers_all_dependencies:
        blockers.append("incomplete_dependency_coverage")
    if not redaction_ok:
        blockers.append("redaction_failed")
    blockers.extend(dependency_blockers)
    if not blockers:
        return "pass", []
    # dependency_blockers arrive prefixed with their dependency label (e.g.
    # "database:gate:restoreConfirmed:fail"), so match on substring rather
    # than a fixed prefix.
    hard = (
        not redaction_ok
        or any("gate:" in b for b in dependency_blockers)
        or any("pause_not_confirmed" in b for b in dependency_blockers)
    )
    return ("fail" if hard else "incomplete"), blockers


# --- Raw manifest / report writer, same shape as soak/report.py -----------


def file_sha256(path: Path) -> str | None:
    if not path.is_file():
        return None
    return hashlib.sha256(path.read_bytes()).hexdigest()


def build_raw_manifest(raw_dir: Path) -> dict[str, Any]:
    files: list[dict[str, Any]] = []
    if not raw_dir.is_dir():
        return {"ok": False, "files": [], "error": "raw_dir_missing"}
    for path in sorted(raw_dir.rglob("*")):
        if not path.is_file() or path.name == "raw-manifest.json":
            continue
        rel = path.relative_to(raw_dir).as_posix()
        files.append(
            {"path": rel, "sha256": file_sha256(path), "bytes": path.stat().st_size}
        )
    return {"ok": bool(files), "files": files}


def write_raw_manifest(raw_dir: Path) -> dict[str, Any]:
    manifest = build_raw_manifest(raw_dir)
    (raw_dir / "raw-manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    manifest["path"] = str(raw_dir / "raw-manifest.json")
    manifest["sha256"] = file_sha256(raw_dir / "raw-manifest.json")
    return manifest


def write_raw(raw_dir: Path, name: str, text: str) -> None:
    raw_dir.mkdir(parents=True, exist_ok=True)
    path = raw_dir / name
    path.write_text(redact.redact_text(text), encoding="utf-8")
    try:
        os.chmod(path, 0o600)
    except OSError:
        pass


def stamp_utc() -> str:
    return time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())


def migration_manifest_sha256() -> str | None:
    return file_sha256(MIGRATIONS_MANIFEST)


def compose_file_sha256() -> str | None:
    return file_sha256(COMPOSE_POC)


def write_reports(out_dir: Path, payload: dict[str, Any]) -> None:
    """Write r06-hanging-soak.json/.md. Refuses to write a report that fails
    its own serialized secret scan (same invariant as soak/report.py)."""
    out_dir.mkdir(parents=True, exist_ok=True)
    canonical = dict(payload)
    canonical["issue"] = ISSUE
    canonical["canonicalReport"] = CANONICAL
    serialized = json.dumps(canonical, indent=2, sort_keys=True) + "\n"
    findings = redact.scan_text(serialized)
    if findings:
        raise RuntimeError("serialized_report_secret_scan_failed")
    (out_dir / CANONICAL).write_text(serialized, encoding="utf-8")
    try:
        os.chmod(out_dir / CANONICAL, 0o600)
    except OSError:
        pass

    status = canonical.get("status", "not_run")
    md = [
        "# P1B-R06 hanging-dependency Compose soak",
        "",
        f"- Status: `{status}`",
        f"- Issue: `{ISSUE}`",
        f"- Canonical JSON: `{CANONICAL}`",
        f"- Raw: `{canonical.get('rawDir')}`",
        "",
        "## Blockers",
        "",
    ]
    blockers = canonical.get("blockers") or []
    md.extend([f"- `{b}`" for b in blockers] or ["- (none)"])
    md.extend(["", "## Dependencies", ""])
    for dep in canonical.get("dependencies") or []:
        md.append(
            f"- `{dep.get('dependencyLabel')}` (`{dep.get('service')}`): "
            f"gates={dep.get('gates')}"
        )
    md.append("")
    (out_dir / "r06-hanging-soak.md").write_text("\n".join(md), encoding="utf-8")
