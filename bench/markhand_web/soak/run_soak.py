#!/usr/bin/env python3
"""P1B-O05 measured mixed-load soak / qualification harness.

Fail-closed:
  - no MARKHAND_SOAK=1 => not_run
  - opt-in without complete prerequisites/evidence => incomplete/fail (never pass)
  - smoke (--duration-seconds != profile 1800) is labeled non-qualifying and cannot pass
  - pass requires exact profile duration 1800 and measured numeric gates

Canonical artifacts:
  bench/markhand_web/reports/phase-1b-gate/o05-soak.{json,md}
  bench/markhand_web/reports/phase-1b-gate/raw/o05-<stamp>/
  summary.json is a thin O05 pointer only (never O04).
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

SOAK_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SOAK_DIR))

import gates_eval  # noqa: E402
import injection  # noqa: E402
import prerequisites  # noqa: E402
import profile as profile_mod  # noqa: E402
import redact  # noqa: E402
import report  # noqa: E402
import sampler  # noqa: E402
import workload  # noqa: E402

ROOT = Path(__file__).resolve().parents[3]
DEFAULT_OUT = ROOT / "bench/markhand_web/reports/phase-1b-gate"
DEFAULT_PROFILE = ROOT / "bench/markhand_web/workloads/phase1b-mixed.yaml"
DEFAULT_GATES = ROOT / "bench/markhand_web/gates.yaml"
F02_BOOT = ROOT / "bench/markhand_web/reports/poc-f02-boot.json"
O02_REPORT = DEFAULT_OUT / "o02-alerts.json"
O03_REPORT = DEFAULT_OUT / "o03-restore.json"
O04_REPORT = DEFAULT_OUT / "o04-release.json"
DEFAULT_COMPOSE_PROJECT = "markhand-poc"
ISSUE = "P1B-O05"
O03_RUNNER = ROOT / "deploy/scripts/o03-bluegreen-restore-drill.sh"


def git_output(*args: str) -> str:
    try:
        return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def migration_manifest_sha256() -> str:
    path = ROOT / "crates/server/migrations/manifest.json"
    return hashlib.sha256(path.read_bytes()).hexdigest()


def compose_project() -> str:
    return (
        os.environ.get("MARKHAND_COMPOSE_PROJECT", DEFAULT_COMPOSE_PROJECT).strip()
        or DEFAULT_COMPOSE_PROJECT
    )


def api_base_url() -> str:
    return os.environ.get("MARKHAND_SOAK_API_BASE", "http://127.0.0.1:8788").rstrip("/")


def stamp_utc() -> str:
    return time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())


def write_raw(raw_dir: Path, name: str, text: str) -> None:
    raw_dir.mkdir(parents=True, exist_ok=True)
    (raw_dir / name).write_text(redact.redact_text(text), encoding="utf-8")


def self_test() -> None:
    # Delegate to unittest module colocated with harness.
    import unittest

    suite = unittest.defaultTestLoader.discover(str(SOAK_DIR), pattern="test_o05_soak.py")
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    if not result.wasSuccessful():
        raise SystemExit(1)
    print("self-test ok")


def run_not_run(args: argparse.Namespace, loaded: dict[str, Any]) -> dict[str, Any]:
    git_short = git_output("rev-parse", "--short", "HEAD")
    git_full = git_output("rev-parse", "HEAD")
    out = Path(args.out)
    raw_dir = out / "raw" / f"o05-{stamp_utc()}"
    raw_dir.mkdir(parents=True, exist_ok=True)
    write_raw(raw_dir, "harness-not-run.txt", "MARKHAND_SOAK!=1; evidence template only\n")
    payload = report.build_not_run_report(
        profile_path=str(Path(args.profile)),
        out_dir=out,
        git_short=git_short,
        git_full=git_full,
        raw_dir=raw_dir,
    )
    payload["profileParsed"] = {
        "name": loaded.get("name"),
        "durationSeconds": loaded.get("durationSeconds"),
        "formats": loaded.get("actors", {}).get("ingest", {}).get("formats"),
    }
    thr = gates_eval.load_thresholds(loaded, Path(args.gates))
    payload["thresholds"] = thr
    status, blockers = report.evaluate_status(
        markhand_soak=False,
        prerequisites_ok=False,
        measured=False,
        smoke=False,
        gates=report.unknown_gates(),
        injection_ok=False,
        redaction_ok=True,
        duration_seconds=0,
        official_duration=int(thr["officialDurationSeconds"]),
    )
    payload["status"] = status
    payload["blockers"] = blockers
    return payload


def run_live(args: argparse.Namespace, loaded: dict[str, Any]) -> dict[str, Any]:
    git_short = git_output("rev-parse", "--short", "HEAD")
    git_full = git_output("rev-parse", "HEAD")
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    stamp = stamp_utc()
    raw_dir = out / "raw" / f"o05-{stamp}"
    raw_dir.mkdir(parents=True, exist_ok=True)

    thr = gates_eval.load_thresholds(loaded, Path(args.gates))
    official = int(thr["officialDurationSeconds"])
    duration = int(args.duration_seconds) if args.duration_seconds is not None else official
    smoke = duration != official
    project = compose_project()

    prereq = prerequisites.validate_prerequisites(
        f02_path=Path(args.f02),
        o02_path=Path(args.o02),
        o03_path=Path(args.o03),
        o04_path=Path(args.o04),
        current_git_full=git_full,
        compose_project=project,
    )
    write_raw(raw_dir, "prerequisites.json", json.dumps(prereq, indent=2) + "\n")

    # Optional same-run O03 restore attestation.
    o03_same_run: dict[str, Any] | None = None
    if args.invoke_o03_restore:
        if not O03_RUNNER.is_file():
            o03_same_run = {"invoked": False, "error": "runner_missing"}
        else:
            proc = subprocess.run(
                ["bash", str(O03_RUNNER)],
                cwd=ROOT,
                capture_output=True,
                text=True,
                check=False,
            )
            write_raw(
                raw_dir,
                "o03-runner.txt",
                f"exit={proc.returncode}\n{proc.stdout}\n{proc.stderr}\n",
            )
            # Re-validate O03 after runner.
            prereq = prerequisites.validate_prerequisites(
                f02_path=Path(args.f02),
                o02_path=Path(args.o02),
                o03_path=Path(args.o03),
                o04_path=Path(args.o04),
                current_git_full=git_full,
                compose_project=project,
            )
            o03_same_run = {"invoked": True, "exitCode": proc.returncode}

    base = api_base_url()
    email = os.environ.get("MARKHAND_SOAK_EMAIL", "admin@poc.example")
    password = os.environ.get("MARKHAND_SOAK_PASSWORD", "")
    collection_id = os.environ.get(
        "MARKHAND_SOAK_COLLECTION_ID", "55555555-5555-5555-5555-555555555501"
    )
    token = os.environ.get("MARKHAND_SOAK_TOKEN", "").strip() or None
    if not token and password:
        try:
            token = workload.login(base, email, password)
        except Exception as exc:  # noqa: BLE001 — recorded as incomplete, not crash
            write_raw(raw_dir, "login-error.txt", f"{type(exc).__name__}\n")
            token = None

    # Never persist password/token to reports. ApiClient paths include /api/v1/...
    host = base
    if host.endswith("/api/v1"):
        host = host[: -len("/api/v1")]
    client = workload.ApiClient(
        host,
        token=token,
        collection_id=collection_id,
        timeout_seconds=float(os.environ.get("MARKHAND_SOAK_TIMEOUT_SECONDS", "30")),
        max_in_flight=int(os.environ.get("MARKHAND_SOAK_MAX_IN_FLIGHT", "32")),
    )

    container_ids: dict[str, str] = {}
    try:
        container_ids = injection.discover_poc_containers(project)
    except injection.InjectionError as exc:
        write_raw(raw_dir, "discover-containers.txt", str(exc) + "\n")

    tracker = sampler.GrowthTracker()
    temp_paths = [
        Path(os.environ.get("MARKHAND_SOAK_TEMP_DIR", "/tmp/markhand-soak")),
        ROOT / "target" / "tmp",
    ]

    def tick(_elapsed: float) -> None:
        stats = sampler.sample_docker_stats(container_ids)
        metrics = sampler.sample_api_metrics(host)
        pg = sampler.sample_pg_connections(
            compose_project=project, container_ids=container_ids
        )
        tracker.observe(
            rss_mb=stats.get("rssMbTotal"),
            temp_bytes=sampler.sample_temp_bytes(temp_paths),
            queue_depth=metrics.get("queueDepthMax"),
            queue_age=metrics.get("queueAgeMax"),
            db_conn=pg.get("connections"),
        )

    measured = False
    stats = None
    load_error = None
    if token and prereq["ok"]:
        try:
            # Initial sample
            tick(0.0)
            stats = workload.run_mixed_load(
                client=client,
                profile=loaded,
                duration_seconds=duration,
                compose_project=project,
                on_tick=tick,
                enable_reconcile=not args.skip_reconcile,
            )
            tick(float(duration))
            measured = True
        except Exception as exc:  # noqa: BLE001
            load_error = f"{type(exc).__name__}:{exc}"
            write_raw(raw_dir, "workload-error.txt", load_error + "\n")
    else:
        write_raw(
            raw_dir,
            "workload-skipped.txt",
            "skipped: missing token and/or prerequisites\n",
        )

    injection_evidence: list[dict[str, Any]] = []
    worker_recovery = None
    dependency_recovery = None
    injection_ok = False
    if args.enable_failure_injection and measured and container_ids:
        try:
            worker_svc = "worker-convert"
            if worker_svc in container_ids:
                ev = injection.kill_and_restart_worker(
                    compose_project=project,
                    service=worker_svc,
                    allowed_ids=container_ids,
                    recovery_deadline_seconds=float(
                        os.environ.get("MARKHAND_SOAK_RECOVERY_DEADLINE", "120")
                    ),
                )
                injection.write_injection_evidence(raw_dir, ev)
                injection_evidence.append(ev)
                worker_recovery = bool(ev.get("recovered"))
                # Refresh ids after restart.
                container_ids = injection.discover_poc_containers(project)
            blip_svc = "postgres"
            blip_seconds = int(loaded["failureInjection"]["dependencyBlipSeconds"])
            if blip_svc in container_ids and blip_seconds > 0:
                ev = injection.dependency_blip(
                    compose_project=project,
                    service=blip_svc,
                    allowed_ids=container_ids,
                    blip_seconds=blip_seconds,
                    recovery_deadline_seconds=float(
                        os.environ.get("MARKHAND_SOAK_RECOVERY_DEADLINE", "180")
                    ),
                )
                injection.write_injection_evidence(raw_dir, ev)
                injection_evidence.append(ev)
                dependency_recovery = bool(ev.get("recovered"))
            injection_ok = worker_recovery is True and dependency_recovery is True
        except injection.InjectionError as exc:
            write_raw(raw_dir, "injection-error.txt", str(exc) + "\n")
            injection_ok = False
            if worker_recovery is None:
                worker_recovery = False
            if dependency_recovery is None:
                dependency_recovery = False
    elif not args.enable_failure_injection:
        write_raw(
            raw_dir,
            "injection-skipped.txt",
            "failure injection disabled; pass requires --enable-failure-injection\n",
        )
        injection_ok = False

    post_restore: dict[str, Any] = {"passed": None}
    if measured and stats is not None:
        post_restore = workload.post_restore_retrieval_check(client, list(stats.deleted_ids))
        write_raw(raw_dir, "post-restore-retrieval.json", json.dumps(post_restore, indent=2) + "\n")

    tracker.write_raw(raw_dir)
    growth = tracker.summary()

    metrics: dict[str, Any] = {}
    if stats is not None:
        metrics = workload.metrics_from_stats(stats, duration)
    metrics.update(
        {
            "rssGrowthMb": (growth.get("rssMb") or {}).get("growth"),
            "tempGrowthMb": (
                None
                if (growth.get("tempBytes") or {}).get("growth") is None
                else round(((growth.get("tempBytes") or {}).get("growth") or 0) / (1024 * 1024), 3)
            ),
            "rss": growth.get("rssMb"),
            "tempBytes": growth.get("tempBytes"),
            "queueDepthMax": growth.get("queueDepthMax"),
            "queueAgeMaxSeconds": growth.get("queueAgeMaxSeconds"),
            "dbConnectionsMax": growth.get("dbConnectionsMax"),
            "smoke": smoke,
            "workerRecoveryPass": worker_recovery,
            "dependencyRecoveryPass": dependency_recovery,
            "postRestoreRetrievalPass": post_restore.get("passed"),
            "requestErrors": metrics.get("requestErrors"),
            "durationSeconds": duration,
        }
    )
    write_raw(raw_dir, "metrics.json", json.dumps(metrics, indent=2) + "\n")

    gates = (
        gates_eval.evaluate_numeric_gates(metrics, thr)
        if measured
        else report.unknown_gates()
    )

    # Image / index provenance (no secrets).
    image_ids = {}
    try:
        image_ids, _digests, _missing = _collect_images(project)
    except Exception:  # noqa: BLE001
        image_ids = {}

    redaction = redact.scan_raw_dir(raw_dir)
    status, blockers = report.evaluate_status(
        markhand_soak=True,
        prerequisites_ok=bool(prereq["ok"]),
        measured=measured,
        smoke=smoke,
        gates=gates,
        injection_ok=injection_ok,
        redaction_ok=bool(redaction.get("passed")),
        duration_seconds=duration,
        official_duration=official,
    )
    if load_error:
        blockers = list(blockers) + [f"workload_error:{load_error}"]
        if status == "pass":
            status = "fail"

    notes = (
        "Smoke/non-qualifying duration; cannot pass official O05."
        if smoke
        else (
            "Live measured soak."
            if status == "pass"
            else "Live soak opted in; see blockers — not a pass."
        )
    )

    payload: dict[str, Any] = {
        "issue": ISSUE,
        "generatedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "status": status,
        "markhandSoak": True,
        "smoke": smoke,
        "smokeNonQualifying": smoke,
        "profile": str(Path(args.profile)),
        "canonicalReport": report.CANONICAL,
        "notes": notes,
        "blockers": blockers,
        "gates": gates,
        "thresholds": thr,
        "metrics": metrics,
        "prerequisites": prereq,
        "failureInjection": {
            "enabled": bool(args.enable_failure_injection),
            "events": injection_evidence,
            "workerRecoveryPass": worker_recovery,
            "dependencyRecoveryPass": dependency_recovery,
        },
        "postRestoreRetrieval": post_restore,
        "o03SameRun": o03_same_run,
        "versions": {
            "git": git_short,
            "gitShaFull": git_full,
            "migrationManifestSha256": migration_manifest_sha256(),
            "indexSignature": os.environ.get("MARKHAND_INDEX_SIGNATURE"),
            "imageIds": image_ids,
            "dockerVersion": _cmd_text(["docker", "--version"]),
            "composeVersion": _cmd_text(["docker", "compose", "version"]),
        },
        "provenance": {
            "gitSha": git_short,
            "gitShaFull": git_full,
            "composeProject": project,
            "apiBase": host,
            "migrationManifestSha256": migration_manifest_sha256(),
            "imageIds": image_ids,
        },
        "redactionScan": redaction,
        "rawDir": str(raw_dir),
        "outDir": str(out),
        "durationSeconds": duration,
        "officialDurationSeconds": official,
    }
    return payload


def _cmd_text(args: list[str]) -> str | None:
    try:
        proc = subprocess.run(args, cwd=ROOT, capture_output=True, text=True, check=False)
    except FileNotFoundError:
        return None
    text = (proc.stdout or proc.stderr or "").strip()
    return text or None


def _collect_images(project: str) -> tuple[dict[str, str], dict[str, str], list[str]]:
    # Reuse discovery; image ids via docker inspect.
    mapping = injection.discover_poc_containers(project)
    ids: dict[str, str] = {}
    digests: dict[str, str] = {}
    for service, cid in mapping.items():
        proc = subprocess.run(
            ["docker", "inspect", "-f", "{{.Image}}", cid],
            capture_output=True,
            text=True,
            check=False,
        )
        image = (proc.stdout or "").strip()
        if image:
            ids[service] = image
    missing = [s for s in prerequisites.EXPECTED_POC_SERVICES if s not in ids]
    return ids, digests, missing


def main() -> int:
    parser = argparse.ArgumentParser(description="P1B-O05 measured soak harness")
    parser.add_argument("--profile", default=str(DEFAULT_PROFILE))
    parser.add_argument("--gates", default=str(DEFAULT_GATES))
    parser.add_argument("--out", default=str(DEFAULT_OUT))
    parser.add_argument(
        "--duration-seconds",
        type=int,
        default=None,
        help="Override duration for smoke only; official pass requires profile 1800 exactly",
    )
    parser.add_argument("--f02", default=str(F02_BOOT))
    parser.add_argument("--o02", default=str(O02_REPORT))
    parser.add_argument("--o03", default=str(O03_REPORT))
    parser.add_argument("--o04", default=str(O04_REPORT))
    parser.add_argument(
        "--enable-failure-injection",
        action="store_true",
        help="Opt-in worker kill + dependency blip (POC project/services only)",
    )
    parser.add_argument(
        "--invoke-o03-restore",
        action="store_true",
        help="Invoke approved O03 runner before evaluating prerequisites",
    )
    parser.add_argument("--skip-reconcile", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument(
        "--validate-report",
        type=Path,
        default=None,
        help="Validate an o05-soak.json and print {status,blockers}",
    )
    args = parser.parse_args()

    if args.self_test:
        self_test()
        return 0

    if args.validate_report is not None:
        payload = json.loads(Path(args.validate_report).read_text(encoding="utf-8"))
        status = payload.get("status")
        blockers = payload.get("blockers") or []
        # Re-evaluate from embedded fields when present.
        if "gates" in payload:
            status, blockers = report.evaluate_status(
                markhand_soak=bool(payload.get("markhandSoak")),
                prerequisites_ok=bool((payload.get("prerequisites") or {}).get("ok")),
                measured=bool(payload.get("metrics")),
                smoke=bool(payload.get("smokeNonQualifying") or payload.get("smoke")),
                gates=payload.get("gates") or report.unknown_gates(),
                injection_ok=bool(
                    (payload.get("failureInjection") or {}).get("workerRecoveryPass")
                )
                and bool(
                    (payload.get("failureInjection") or {}).get("dependencyRecoveryPass")
                ),
                redaction_ok=bool((payload.get("redactionScan") or {}).get("passed")),
                duration_seconds=int(payload.get("durationSeconds") or 0),
                official_duration=int(
                    payload.get("officialDurationSeconds")
                    or gates_eval.OFFICIAL_DURATION_SECONDS
                ),
            )
        print(json.dumps({"status": status, "blockers": blockers}, indent=2, sort_keys=True))
        return 0 if status == "pass" else 1

    loaded = profile_mod.load_workload_profile(args.profile)
    if os.environ.get("MARKHAND_SOAK") != "1":
        payload = run_not_run(args, loaded)
    else:
        payload = run_live(args, loaded)

    report.write_reports(Path(args.out), payload)
    print(Path(args.out) / report.CANONICAL)
    return 0 if payload.get("status") == "pass" else (0 if payload.get("status") == "not_run" else 1)


if __name__ == "__main__":
    raise SystemExit(main())
