#!/usr/bin/env python3
"""P1B-R06 hanging-dependency Compose soak harness.

Proves the readiness contract in `crates/server/src/services/readiness.rs`
against the *real* POC Compose stack while a dependency is paused (hung),
not stopped: `/api/v1/health/ready` must fail with the exact in-progress
probe code within the outer deadline, `/api/v1/health/live` and
`/api/v1/openapi.yaml` (routes that never touch a dependency probe) must
stay within their own budget, and nothing may queue without bound behind
the paused socket. See `bench/markhand_web/hanging_soak/README.md` and
`docs/runbooks/phase-1b/hanging-dependency-soak.md`.

Fail-closed, mirroring `bench/markhand_web/soak/run_soak.py` (P1B-O05):
  - no `MARKHAND_HANGING_SOAK=1` => not_run
  - opt-in without complete evidence => incomplete/fail, never pass
  - `--sustain-seconds` below `gate_report.MIN_QUALIFYING_SUSTAIN_SECONDS`
    (or a `--dependencies` subset) is labeled non-qualifying and cannot pass

Canonical artifacts:
  bench/markhand_web/reports/phase-1b-gate/r06-hanging-soak.{json,md}
  bench/markhand_web/reports/phase-1b-gate/raw/r06-<stamp>/
"""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import os
import subprocess
import sys
import time
import uuid
from pathlib import Path
from typing import Any
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

SELF_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SELF_DIR))

import failpoint  # noqa: E402
import gate_report  # noqa: E402

ROOT = Path(__file__).resolve().parents[3]
DEFAULT_OUT = gate_report.OUT_DIR
DEFAULT_COMPOSE_PROJECT = "markhand-poc"
PROVENANCE_SERVICES = ("api", "postgres", "qdrant", "minio")


def git_output(*args: str) -> str:
    try:
        return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def git_clean_tree() -> bool:
    try:
        proc = subprocess.run(
            ["git", "status", "--porcelain"],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=False,
            timeout=15,
        )
    except (OSError, subprocess.SubprocessError):
        return False
    return proc.returncode == 0 and not (proc.stdout or "").strip()


def compose_project() -> str:
    return (
        os.environ.get("MARKHAND_COMPOSE_PROJECT", DEFAULT_COMPOSE_PROJECT).strip()
        or DEFAULT_COMPOSE_PROJECT
    )


def api_base_url() -> str:
    port = os.environ.get("MARKHAND_API_PORT", "8788")
    return os.environ.get("MARKHAND_HANGING_SOAK_API_BASE", f"http://127.0.0.1:{port}").rstrip("/")


def make_raw_run_dir(out: Path) -> Path:
    raw_base = out / "raw"
    raw_base.mkdir(parents=True, exist_ok=True)
    for _ in range(10):
        candidate = raw_base / f"r06-{gate_report.stamp_utc()}-{uuid.uuid4().hex[:8]}"
        try:
            candidate.mkdir(mode=0o700, parents=False, exist_ok=False)
            return candidate
        except FileExistsError:
            continue
    raise RuntimeError("raw_run_dir_collision")


# --- HTTP sampling (dependency-injectable opener for hermetic self-test) ---


def sample_endpoint(
    url: str,
    *,
    timeout_seconds: float,
    opener: Any = None,
) -> dict[str, Any]:
    """GET url with a hard client-side timeout. Never raises: a hang, a
    connection error, and a clean HTTP error are all distinguishable results,
    not process crashes."""
    req = Request(url, method="GET", headers={"Accept": "application/json"})
    open_fn = opener or urlopen
    start = time.monotonic()
    try:
        response = open_fn(req, timeout=timeout_seconds)
        try:
            status = response.getcode()
            body_bytes = response.read()
        finally:
            response.close()
        elapsed = time.monotonic() - start
        probe_code = _extract_probe_code(body_bytes)
        return {
            "httpStatus": status,
            "elapsedSeconds": round(elapsed, 3),
            "probeCode": probe_code,
            "error": None,
        }
    except HTTPError as exc:
        elapsed = time.monotonic() - start
        try:
            body_bytes = exc.read()
        except Exception:  # noqa: BLE001
            body_bytes = b""
        return {
            "httpStatus": exc.code,
            "elapsedSeconds": round(elapsed, 3),
            "probeCode": _extract_probe_code(body_bytes),
            "error": None,
        }
    except (URLError, TimeoutError, OSError) as exc:
        elapsed = time.monotonic() - start
        return {
            "httpStatus": None,
            "elapsedSeconds": round(elapsed, 3),
            "probeCode": None,
            "error": type(exc).__name__,
        }


def _extract_probe_code(body_bytes: bytes) -> str | None:
    try:
        body = json.loads(body_bytes.decode("utf-8"))
    except (ValueError, UnicodeDecodeError, AttributeError):
        return None
    if not isinstance(body, dict):
        return None
    details = body.get("details")
    if not isinstance(details, dict):
        return None
    probe = details.get("probe")
    return probe if isinstance(probe, str) else None


def run_concurrency_batch(
    url: str,
    *,
    n: int,
    timeout_seconds: float,
    opener: Any = None,
) -> dict[str, Any]:
    start = time.monotonic()
    results: list[dict[str, Any]] = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, n)) as pool:
        futures = [
            pool.submit(sample_endpoint, url, timeout_seconds=timeout_seconds, opener=opener)
            for _ in range(n)
        ]
        for fut in futures:
            results.append(fut.result())
    span = time.monotonic() - start
    return {"n": n, "spanSeconds": round(span, 3), "results": results}


# --- Live per-dependency run ------------------------------------------------


def run_dependency(
    *,
    label: str,
    api_base: str,
    compose_project_name: str,
    sustain_seconds: float,
    poll_interval_seconds: float,
    concurrency: int,
    recovery_deadline_seconds: float,
    raw_dir: Path,
    runner: failpoint.Runner | None = None,
) -> dict[str, Any]:
    spec = gate_report.DEPENDENCY_PROBES[label]
    expected_code = spec["expectedCode"]
    result: dict[str, Any] = {
        "dependencyLabel": label,
        "expectedProbeCode": expected_code,
        "service": None,
        "pauseConfirmed": False,
        "baseline": {},
        "samples": {"ready": [], "live": [], "openapi": []},
        "concurrencyBatches": [],
        "restore": None,
        "recovery": None,
        "blockers": [],
    }

    ready_url = f"{api_base}/api/v1/health/ready"
    live_url = f"{api_base}/api/v1/health/live"
    openapi_url = f"{api_base}/api/v1/openapi.yaml"

    try:
        container_ids = failpoint.discover_poc_containers(
            compose_project_name, runner=runner
        )
    except failpoint.FailpointError as exc:
        result["blockers"].append(f"discover_containers_failed:{label}:{exc}")
        return result

    service = next((s for s in spec["services"] if s in container_ids), None)
    result["service"] = service
    if service is None:
        result["blockers"].append(f"precondition_not_running:{label}")
        return result

    try:
        cid = failpoint.resolve_target_container(
            service=service, allowed_ids=container_ids
        )
    except failpoint.FailpointError as exc:
        result["blockers"].append(f"precondition_failed:{label}:{exc}")
        return result

    ready_timeout = gate_report.READY_BOUND_SECONDS + 1.0
    live_timeout = gate_report.LIVE_BUDGET_SECONDS + 1.0

    baseline_ready = sample_endpoint(ready_url, timeout_seconds=ready_timeout)
    result["baseline"] = {
        "ready": baseline_ready,
        "live": sample_endpoint(live_url, timeout_seconds=live_timeout),
        "openapi": sample_endpoint(openapi_url, timeout_seconds=live_timeout),
    }
    if baseline_ready.get("httpStatus") != 200:
        # Fail closed: a code change during the pause can only be attributed
        # to the dependency under test if readiness was healthy beforehand.
        result["blockers"].append(f"baseline_ready_not_healthy:{label}")
        return result

    guard = failpoint.PauseGuard(service=service, container_id=cid, runner=runner)
    try:
        guard.arm_and_pause()
        result["pauseConfirmed"] = guard.pause_confirmed
        deadline = time.monotonic() + sustain_seconds
        batch_every_ticks = max(1, round(10.0 / max(poll_interval_seconds, 0.5)))
        tick = 0
        while time.monotonic() < deadline:
            result["samples"]["ready"].append(
                sample_endpoint(ready_url, timeout_seconds=ready_timeout)
            )
            result["samples"]["live"].append(
                sample_endpoint(live_url, timeout_seconds=live_timeout)
            )
            result["samples"]["openapi"].append(
                sample_endpoint(openapi_url, timeout_seconds=live_timeout)
            )
            if tick % batch_every_ticks == 0:
                result["concurrencyBatches"].append(
                    run_concurrency_batch(
                        ready_url, n=concurrency, timeout_seconds=ready_timeout
                    )
                )
            tick += 1
            remaining = deadline - time.monotonic()
            if remaining > 0:
                time.sleep(min(poll_interval_seconds, remaining))
    finally:
        result["restore"] = guard.restore_if_armed()

    gate_report.write_raw(
        raw_dir, f"{label}-samples.json", json.dumps(result, indent=2, default=str) + "\n"
    )

    recovery_start = time.monotonic()
    recovered = False
    recovery_seconds = None
    while time.monotonic() - recovery_start < recovery_deadline_seconds:
        sample = sample_endpoint(ready_url, timeout_seconds=ready_timeout)
        if sample.get("httpStatus") == baseline_ready.get("httpStatus"):
            recovered = True
            recovery_seconds = round(time.monotonic() - recovery_start, 3)
            break
        time.sleep(1.0)
    result["recovery"] = {
        "recoveredWithinDeadline": recovered,
        "recoverySeconds": recovery_seconds,
        "deadlineSeconds": recovery_deadline_seconds,
    }

    evaluation = gate_report.evaluate_dependency(result)
    result["gates"] = evaluation["gates"]
    result["blockers"].extend(evaluation["blockers"])
    return result


def _image_ids(project: str) -> dict[str, str]:
    ids: dict[str, str] = {}
    try:
        container_ids = failpoint.discover_poc_containers(project)
    except failpoint.FailpointError:
        return ids
    for service in PROVENANCE_SERVICES:
        cid = container_ids.get(service)
        if not cid:
            continue
        proc = subprocess.run(
            ["docker", "inspect", "-f", "{{.Image}}", cid],
            capture_output=True,
            text=True,
            check=False,
            timeout=15,
        )
        if proc.returncode == 0:
            image = (proc.stdout or "").strip()
            if image:
                ids[service] = image
    return ids


def run_not_run(args: argparse.Namespace) -> dict[str, Any]:
    out = Path(args.out)
    raw_dir = make_raw_run_dir(out)
    gate_report.write_raw(
        raw_dir,
        "harness-not-run.txt",
        "MARKHAND_HANGING_SOAK!=1; evidence template only\n",
    )
    raw_manifest = gate_report.write_raw_manifest(raw_dir)
    status, blockers = gate_report.evaluate_status(
        opted_in=False,
        smoke=False,
        covers_all_dependencies=False,
        dependency_blockers=[],
        redaction_ok=True,
    )
    return {
        "issue": gate_report.ISSUE,
        "generatedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "status": status,
        "markhandHangingSoak": False,
        "smoke": False,
        "canonicalReport": gate_report.CANONICAL,
        "notes": "Stack not opted in; report records harness intent only",
        "blockers": blockers,
        "dependencies": [],
        "versions": {"gitShaFull": git_output("rev-parse", "HEAD")},
        "provenance": {"gitShaFull": git_output("rev-parse", "HEAD")},
        "redactionScan": {"passed": True, "findings": []},
        "rawManifest": raw_manifest,
        "rawDir": str(raw_dir),
        "outDir": str(out),
    }


def run_live(args: argparse.Namespace) -> dict[str, Any]:
    out = Path(args.out)
    raw_dir = make_raw_run_dir(out)
    project = compose_project()
    api_base = api_base_url()
    git_full = git_output("rev-parse", "HEAD")
    git_short = git_output("rev-parse", "--short", "HEAD")

    requested = (
        [d.strip() for d in args.dependencies.split(",") if d.strip()]
        if args.dependencies
        else list(gate_report.DEPENDENCY_PROBES)
    )
    unknown = [d for d in requested if d not in gate_report.DEPENDENCY_PROBES]
    if unknown:
        raise SystemExit(f"unknown --dependencies entries: {unknown}")
    covers_all = sorted(requested) == sorted(gate_report.DEPENDENCY_PROBES)

    smoke = bool(args.sustain_seconds < gate_report.MIN_QUALIFYING_SUSTAIN_SECONDS)

    dependencies: list[dict[str, Any]] = []
    dependency_blockers: list[str] = []
    for label in requested:
        dep_result = run_dependency(
            label=label,
            api_base=api_base,
            compose_project_name=project,
            sustain_seconds=args.sustain_seconds,
            poll_interval_seconds=args.poll_interval_seconds,
            concurrency=args.concurrency,
            recovery_deadline_seconds=args.recovery_deadline_seconds,
            raw_dir=raw_dir,
        )
        dependencies.append(dep_result)
        dependency_blockers.extend(
            f"{label}:{b}" for b in dep_result.get("blockers", [])
        )

    raw_manifest = gate_report.write_raw_manifest(raw_dir)
    redaction = {"passed": True, "findings": []}
    for path in raw_dir.rglob("*"):
        if path.is_file() and path.name != "raw-manifest.json":
            findings = gate_report.redact.scan_text(
                path.read_text(encoding="utf-8", errors="replace")
            )
            if findings:
                redaction = {"passed": False, "findings": findings}

    status, blockers = gate_report.evaluate_status(
        opted_in=True,
        smoke=smoke,
        covers_all_dependencies=covers_all,
        dependency_blockers=dependency_blockers,
        redaction_ok=bool(redaction.get("passed")),
    )

    image_ids = _image_ids(project)
    return {
        "issue": gate_report.ISSUE,
        "generatedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "status": status,
        "markhandHangingSoak": True,
        "smoke": smoke,
        "smokeNonQualifying": smoke,
        "canonicalReport": gate_report.CANONICAL,
        "sustainSeconds": args.sustain_seconds,
        "minQualifyingSustainSeconds": gate_report.MIN_QUALIFYING_SUSTAIN_SECONDS,
        "dependenciesRequested": requested,
        "coversAllDependencies": covers_all,
        "notes": (
            "Smoke/non-qualifying run (sustain below the minimum, or a "
            "restricted --dependencies subset); cannot pass."
            if smoke or not covers_all
            else ("Live measured hanging-dependency soak." if status == "pass" else
                  "Live hanging-dependency soak opted in; see blockers — not a pass.")
        ),
        "blockers": blockers,
        "dependencies": dependencies,
        "versions": {
            "git": git_short,
            "gitShaFull": git_full,
            "migrationManifestSha256": gate_report.migration_manifest_sha256(),
            "composeFileSha256": gate_report.compose_file_sha256(),
            "imageIds": image_ids,
        },
        "provenance": {
            "gitSha": git_short,
            "gitShaFull": git_full,
            "gitDirty": not git_clean_tree(),
            "composeProject": project,
            "apiBase": api_base,
            "migrationManifestSha256": gate_report.migration_manifest_sha256(),
            "composeFileSha256": gate_report.compose_file_sha256(),
            "imageIds": image_ids,
        },
        "redactionScan": redaction,
        "rawManifest": raw_manifest,
        "rawDir": str(raw_dir),
        "outDir": str(out),
    }


def self_test() -> None:
    import unittest

    suite = unittest.defaultTestLoader.discover(
        str(SELF_DIR), pattern="test_hanging_soak.py"
    )
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    if not result.wasSuccessful():
        raise SystemExit(1)
    print("self-test ok")


def main() -> int:
    parser = argparse.ArgumentParser(
        description="P1B-R06 hanging-dependency Compose soak harness"
    )
    parser.add_argument("--out", default=str(DEFAULT_OUT))
    parser.add_argument(
        "--sustain-seconds", type=float, default=float(gate_report.DEFAULT_SUSTAIN_SECONDS)
    )
    parser.add_argument(
        "--poll-interval-seconds",
        type=float,
        default=gate_report.DEFAULT_POLL_INTERVAL_SECONDS,
    )
    parser.add_argument("--concurrency", type=int, default=gate_report.DEFAULT_CONCURRENCY)
    parser.add_argument(
        "--recovery-deadline-seconds",
        type=float,
        default=gate_report.DEFAULT_RECOVERY_DEADLINE_SECONDS,
    )
    parser.add_argument(
        "--dependencies",
        default=None,
        help="Comma list restricting which dependency labels to run "
        f"(default: all of {sorted(gate_report.DEPENDENCY_PROBES)}); a "
        "restricted run is always non-qualifying (cannot pass)",
    )
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        self_test()
        return 0

    if os.environ.get("MARKHAND_HANGING_SOAK") != "1":
        payload = run_not_run(args)
    else:
        payload = run_live(args)

    gate_report.write_reports(Path(args.out), payload)
    print(Path(args.out) / gate_report.CANONICAL)
    return (
        0
        if payload.get("status") in ("pass", "not_run")
        else 1
    )


if __name__ == "__main__":
    raise SystemExit(main())
