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

import fixtures  # noqa: E402
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
DEFAULT_SAMPLE_INTERVAL = 5.0


def git_output(*args: str) -> str:
    try:
        return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def migration_manifest_sha256() -> str:
    path = ROOT / "crates/server/migrations/manifest.json"
    return hashlib.sha256(path.read_bytes()).hexdigest()


def compose_file_sha256() -> str:
    path = ROOT / "deploy/compose.poc.yml"
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
    # Always ensure fixtures exist even for not_run template (documents intent).
    try:
        fixtures.preflight_fixtures(loaded["actors"]["ingest"]["formats"])
        fixture_note = "fixtures_preflight_ok"
    except fixtures.FixtureError as exc:
        fixture_note = f"fixtures_preflight_failed:{exc}"
        write_raw(raw_dir, "fixtures-preflight.txt", fixture_note + "\n")
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
    payload["fixturePreflight"] = fixture_note
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


def _collect_images(project: str) -> tuple[dict[str, str], dict[str, str], list[str]]:
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


def _cmd_text(args: list[str]) -> str | None:
    try:
        proc = subprocess.run(args, cwd=ROOT, capture_output=True, text=True, check=False)
    except FileNotFoundError:
        return None
    text = (proc.stdout or proc.stderr or "").strip()
    return text or None



def run_live(args: argparse.Namespace, loaded: dict[str, Any]) -> dict[str, Any]:
    import dataset as dataset_mod

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
    formats = list(loaded["actors"]["ingest"]["formats"])
    modes = list(loaded["actors"]["query"]["modes"])

    architectural_blockers: list[str] = []

    # 1) Converter-accepted fixture preflight (fail closed).
    try:
        fixture_info = fixtures.preflight_fixtures(formats, generate=True)
        write_raw(raw_dir, "fixtures-preflight.json", json.dumps(fixture_info, indent=2) + "\n")
        fixture_ok = True
    except fixtures.FixtureError as exc:
        write_raw(raw_dir, "fixtures-preflight.txt", str(exc) + "\n")
        fixture_ok = False
        fixture_info = {"ok": False, "error": str(exc)}
        architectural_blockers.append("fixtures_preflight_failed")

    container_ids: dict[str, str] = {}
    try:
        container_ids = injection.discover_poc_containers(project)
    except injection.InjectionError as exc:
        write_raw(raw_dir, "discover-containers.txt", str(exc) + "\n")

    image_ids: dict[str, str] = {}
    try:
        image_ids, _d, _m = _collect_images(project)
    except Exception:  # noqa: BLE001
        image_ids = {}

    index_sig = os.environ.get("MARKHAND_INDEX_SIGNATURE", "").strip() or None
    prereq = prerequisites.validate_prerequisites(
        f02_path=Path(args.f02),
        o02_path=Path(args.o02),
        o03_path=Path(args.o03),
        o04_path=Path(args.o04),
        current_git_full=git_full,
        compose_project=project,
        live_image_ids=image_ids or None,
        live_index_signature=index_sig,
    )
    write_raw(raw_dir, "prerequisites.json", json.dumps(prereq, indent=2) + "\n")

    host = api_base_url()
    if host.endswith("/api/v1"):
        host = host[: -len("/api/v1")]
    email = os.environ.get("MARKHAND_SOAK_EMAIL", "admin@poc.example")
    password = os.environ.get("MARKHAND_SOAK_PASSWORD", "")
    collection_id = os.environ.get(
        "MARKHAND_SOAK_COLLECTION_ID", "55555555-5555-5555-5555-555555555501"
    )
    token = os.environ.get("MARKHAND_SOAK_TOKEN", "").strip() or None
    if not token and password:
        try:
            token = workload.login(host, email, password)
        except Exception as exc:  # noqa: BLE001
            write_raw(raw_dir, "login-error.txt", f"{type(exc).__name__}\n")
            token = None

    client = workload.ApiClient(
        host,
        token=token,
        collection_id=collection_id,
        timeout_seconds=float(os.environ.get("MARKHAND_SOAK_TIMEOUT_SECONDS", "30")),
        max_in_flight=int(os.environ.get("MARKHAND_SOAK_MAX_IN_FLIGHT", "32")),
    )

    # 2) Compare dataset — fail closed if profile includes compare.
    compare_info = dataset_mod.resolve_compare_or_block(
        client if token else None, modes=modes
    )
    write_raw(raw_dir, "compare-dataset.json", json.dumps(compare_info, indent=2) + "\n")
    if compare_info.get("required") and not compare_info.get("available"):
        architectural_blockers.append(
            compare_info.get("blocker") or "compare_dataset_unavailable"
        )

    # Seed + wait indexed before timed schedule.
    seed_info: dict[str, Any] = {"ok": False}
    retained_ids: list[str] = []
    baseline_ids: list[str] = []
    can_seed = bool(token and fixture_ok and prereq["ok"] and compare_info.get("available", True))
    if can_seed:
        try:
            seed_info = dataset_mod.seed_and_wait_indexed(
                client,
                formats=formats,
                fixture_path_fn=fixtures.fixture_path,
                timeout_seconds=float(os.environ.get("MARKHAND_SOAK_SEED_TIMEOUT", "180")),
            )
            retained_ids = list(seed_info.get("retainedDocumentIds") or [])
            baseline_ids = list(retained_ids)
            write_raw(raw_dir, "seed.json", json.dumps(seed_info, indent=2) + "\n")
        except dataset_mod.DatasetError as exc:
            seed_info = {"ok": False, "error": str(exc)}
            write_raw(raw_dir, "seed-error.txt", str(exc) + "\n")
            architectural_blockers.append("seed_index_unavailable")
            can_seed = False
    else:
        write_raw(raw_dir, "seed-skipped.txt", "seed skipped: missing token/prereq/fixture/compare\n")

    # Injection plan (async executor).
    kill_every = int(loaded["failureInjection"].get("killWorkerEverySeconds") or 0)
    blip_seconds = int(loaded["failureInjection"].get("dependencyBlipSeconds") or 0)
    injection_schedule: list[tuple[float, str]] = []
    if args.enable_failure_injection and kill_every > 0:
        t = float(kill_every)
        while t < duration:
            injection_schedule.append((t, "kill_worker"))
            t += float(kill_every)
    if args.enable_failure_injection and blip_seconds > 0:
        blip_at = min(float(duration) * 0.5, max(1.0, float(duration) - blip_seconds - 1))
        injection_schedule.append((blip_at, "dependency_blip"))

    plan = injection.InjectionPlan()
    recovery_deadline = float(os.environ.get("MARKHAND_SOAK_RECOVERY_DEADLINE", "120"))
    allowed_ids_holder: dict[str, str] = dict(container_ids)

    def make_kill():
        ids = dict(allowed_ids_holder)
        return injection.kill_and_restart_worker(
            compose_project=project,
            service="worker-convert",
            allowed_ids=ids,
            recovery_deadline_seconds=recovery_deadline,
        )

    def make_blip():
        ids = dict(allowed_ids_holder)
        return injection.dependency_blip(
            compose_project=project,
            service="postgres",
            allowed_ids=ids,
            blip_seconds=blip_seconds,
            recovery_deadline_seconds=recovery_deadline,
        )

    def injection_callback(elapsed: float, kind: str) -> None:
        # Non-blocking: schedule onto dedicated executor.
        if kind == "kill_worker":
            plan.schedule(kind=kind, scheduled_at=elapsed, fn=make_kill)
        elif kind == "dependency_blip":
            plan.schedule(kind=kind, scheduled_at=elapsed, fn=make_blip)

    tracker = sampler.GrowthTracker()
    sample_interval = float(
        os.environ.get("MARKHAND_SOAK_SAMPLE_INTERVAL_SECONDS", str(DEFAULT_SAMPLE_INTERVAL))
    )

    def sample_once() -> None:
        stats_s = sampler.sample_docker_stats(allowed_ids_holder)
        metrics_s = sampler.sample_api_metrics(host)
        pg = sampler.sample_pg_connections(
            compose_project=project, container_ids=allowed_ids_holder
        )
        temp = sampler.sample_container_temp_bytes(allowed_ids_holder)
        tracker.observe(
            rss_mb=stats_s.get("rssMbTotal"),
            temp_bytes=temp.get("tempBytes"),
            queue_depth=metrics_s.get("queueDepthMax"),
            queue_age=metrics_s.get("queueAgeMax"),
            db_conn=pg.get("connections"),
        )

    bg = sampler.BackgroundSampler(interval_seconds=sample_interval, sample_fn=sample_once)

    measured = False
    stats = None
    load_error = None
    injection_summary: dict[str, Any] = {"ok": False}
    can_run = bool(can_seed and seed_info.get("ok") and not architectural_blockers)
    if can_run:
        try:
            sample_once()
            bg.start()
            plan.workload_start_mono = time.monotonic()
            if args.enable_failure_injection:
                plan.start_pool(max_workers=2)
            stats = workload.run_mixed_load(
                client=client,
                profile=loaded,
                duration_seconds=duration,
                compose_project=project,
                enable_reconcile=not args.skip_reconcile,
                injection_callback=injection_callback if args.enable_failure_injection else None,
                injection_schedule=injection_schedule if args.enable_failure_injection else None,
                compare_dataset=compare_info.get("dataset"),
                injection_window_fn=plan.in_window,
                retained_ids=retained_ids,
                skip_fixture_preflight=True,
            )
            measured = True
            if args.enable_failure_injection:
                try:
                    injection_summary = plan.join(timeout=recovery_deadline + blip_seconds + 60)
                    write_raw(
                        raw_dir,
                        "injection-summary.json",
                        json.dumps(injection_summary, indent=2) + "\n",
                    )
                    for ev in injection_summary.get("events") or []:
                        injection.write_injection_evidence(raw_dir, ev)
                    # Refresh container ids after mutations.
                    try:
                        allowed_ids_holder.update(injection.discover_poc_containers(project))
                    except injection.InjectionError:
                        pass
                except injection.InjectionError as exc:
                    injection_summary = {"ok": False, "error": str(exc)}
                    write_raw(raw_dir, "injection-error.txt", str(exc) + "\n")
        except Exception as exc:  # noqa: BLE001
            load_error = f"{type(exc).__name__}:{exc}"
            write_raw(raw_dir, "workload-error.txt", load_error + "\n")
        finally:
            bg.stop()
            sample_once()
    else:
        write_raw(
            raw_dir,
            "workload-skipped.txt",
            "skipped: " + ",".join(architectural_blockers or ["not_ready"]) + "\n",
        )

    injection_ok = bool(args.enable_failure_injection and injection_summary.get("ok"))
    if not args.enable_failure_injection:
        write_raw(
            raw_dir,
            "injection-skipped.txt",
            "failure injection disabled; pass requires --enable-failure-injection\n",
        )

    # Same-run O03 checkpoint AFTER baseline (does not cut over blue API).
    o03_same_run: dict[str, Any] | None = None
    o03_report = None
    if Path(args.o03).is_file():
        try:
            o03_report = json.loads(Path(args.o03).read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            o03_report = None
    same_run_restore = False
    if measured and args.invoke_o03_restore:
        if not O03_RUNNER.is_file():
            o03_same_run = {"invoked": False, "error": "runner_missing", "sameRun": False}
        else:
            # Capture immutable baseline IDs before backup/restore.
            write_raw(
                raw_dir,
                "baseline-dataset-ids.json",
                json.dumps({"retained": baseline_ids, "seeded": seed_info.get("seeded")}, indent=2)
                + "\n",
            )
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
            same_run_restore = proc.returncode == 0
            o03_same_run = {
                "invoked": True,
                "exitCode": proc.returncode,
                "sameRun": same_run_restore,
                "phase": "post_baseline_checkpoint",
            }
            if Path(args.o03).is_file():
                try:
                    o03_report = json.loads(Path(args.o03).read_text(encoding="utf-8"))
                except (OSError, json.JSONDecodeError):
                    pass

    restored_info = dataset_mod.resolve_restored_api_base(
        blue_base=host, o03_report=o03_report if isinstance(o03_report, dict) else None
    )
    write_raw(raw_dir, "restored-api.json", json.dumps(restored_info, indent=2) + "\n")
    # Post-restore proof requires a distinct green endpoint. O03 exit 0 alone
    # is not enough; blue API must never masquerade as restored.
    if args.invoke_o03_restore or same_run_restore:
        if not restored_info.get("available"):
            architectural_blockers.append(
                restored_info.get("blocker") or "restored_api_base_missing"
            )

    post_restore: dict[str, Any] = {
        "passed": None,
        "gate": "unknown",
        "reason": "no_reachable_restored_endpoint",
    }
    if measured and stats is not None:
        restored_client = None
        unauthorized_client = None
        if restored_info.get("available"):
            restored_host = restored_info["restoredApiBase"]
            # Auth against restored endpoint (token may need re-login).
            restored_token = token
            if password:
                try:
                    restored_token = workload.login(restored_host, email, password)
                except Exception:  # noqa: BLE001
                    restored_token = token
            restored_client = workload.ApiClient(
                restored_host,
                token=restored_token,
                collection_id=collection_id,
                timeout_seconds=float(os.environ.get("MARKHAND_SOAK_TIMEOUT_SECONDS", "30")),
            )
            unauthorized_client = workload.ApiClient(
                restored_host,
                token="invalid-unauthorized-token",
                collection_id=collection_id,
                timeout_seconds=10.0,
            )
        post_restore = dataset_mod.post_restore_retrieval_check(
            restored_client or client,
            retained_ids=list(stats.retained_ids or baseline_ids),
            deleted_ids=list(stats.deleted_ids),
            unauthorized_client=unauthorized_client,
            same_run_restore=same_run_restore,
            restored_endpoint_ok=bool(restored_info.get("available")),
        )
        write_raw(raw_dir, "post-restore-retrieval.json", json.dumps(post_restore, indent=2) + "\n")

    tracker.write_raw(raw_dir)
    growth = tracker.summary()

    metrics: dict[str, Any] = {"measured": measured}
    completeness = {"passed": None}
    if stats is not None:
        metrics.update(workload.metrics_from_stats(stats, duration, modes=modes))
        completeness = workload.completeness_ok(stats, ratio=float(thr["completenessRatio"]))
        metrics["completenessPassed"] = completeness.get("passed")
        metrics["completeness"] = completeness
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
            "workerRecoveryPass": (
                injection_summary.get("allRecovered")
                if args.enable_failure_injection
                else None
            ),
            "dependencyRecoveryPass": (
                injection_summary.get("allRecovered")
                if args.enable_failure_injection
                else None
            ),
            "postRestoreRetrievalPass": post_restore.get("passed"),
            "durationSeconds": duration,
            "samplerErrors": list(bg.errors),
            "architecturalBlockers": architectural_blockers,
        }
    )
    # Prefer kill/blip specific recovery flags when available.
    if args.enable_failure_injection and injection_summary.get("events"):
        kills = [e for e in injection_summary["events"] if e.get("action") == "kill_worker"]
        blips = [e for e in injection_summary["events"] if e.get("action") == "dependency_blip"]
        metrics["workerRecoveryPass"] = bool(kills) and all(e.get("recovered") for e in kills)
        metrics["dependencyRecoveryPass"] = bool(blips) and all(e.get("recovered") for e in blips)

    write_raw(raw_dir, "metrics.json", json.dumps(metrics, indent=2) + "\n")

    gates = (
        gates_eval.evaluate_numeric_gates(metrics, thr)
        if measured
        else report.unknown_gates()
    )
    if architectural_blockers:
        for key in ("queryP95", "queryP99", "postRestoreRetrieval", "completeness"):
            if key in gates and gates[key] == "pass":
                gates[key] = "fail" if measured else "unknown"
        if "compare_dataset_unavailable" in architectural_blockers:
            gates["queryP95"] = "fail" if measured else "unknown"
            gates["queryP99"] = "fail" if measured else "unknown"
        if any(b.startswith("restored_api") for b in architectural_blockers):
            gates["postRestoreRetrieval"] = "unknown"

    redaction = redact.scan_raw_dir(raw_dir)
    status, blockers = report.evaluate_status(
        markhand_soak=True,
        prerequisites_ok=bool(prereq["ok"]) and fixture_ok and not architectural_blockers,
        measured=measured,
        smoke=smoke,
        gates=gates,
        injection_ok=injection_ok,
        redaction_ok=bool(redaction.get("passed")),
        duration_seconds=duration,
        official_duration=official,
    )
    blockers = list(blockers) + [f"arch:{b}" for b in architectural_blockers]
    if load_error:
        blockers.append(f"workload_error:{load_error}")
        if status == "pass":
            status = "fail"
    if status == "pass" and architectural_blockers:
        status = "fail"

    notes = (
        "Smoke/non-qualifying duration; cannot pass official O05."
        if smoke
        else (
            "Live measured soak."
            if status == "pass"
            else "Live soak opted in; see blockers — not a pass. "
            "Architectural compare/restore cutover blockers are documented in soak-o05.md."
        )
    )

    return {
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
        "architecturalBlockers": architectural_blockers,
        "gates": gates,
        "thresholds": thr,
        "metrics": metrics,
        "prerequisites": prereq,
        "fixturePreflight": fixture_info,
        "compareDataset": compare_info,
        "seed": seed_info,
        "failureInjection": {
            "enabled": bool(args.enable_failure_injection),
            "schedule": [{"at": t, "kind": k} for t, k in injection_schedule],
            "summary": injection_summary,
        },
        "postRestoreRetrieval": post_restore,
        "restoredApi": restored_info,
        "o03SameRun": o03_same_run,
        "versions": {
            "git": git_short,
            "gitShaFull": git_full,
            "migrationManifestSha256": migration_manifest_sha256(),
            "composeFileSha256": compose_file_sha256(),
            "indexSignature": index_sig,
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
            "composeFileSha256": compose_file_sha256(),
            "imageIds": image_ids,
            "indexSignature": index_sig,
        },
        "redactionScan": redaction,
        "rawDir": str(raw_dir),
        "outDir": str(out),
        "durationSeconds": duration,
        "officialDurationSeconds": official,
        "sampleIntervalSeconds": sample_interval,
    }



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
        help="Opt-in worker kill + dependency blip during active workload",
    )
    parser.add_argument(
        "--invoke-o03-restore",
        action="store_true",
        help="Invoke approved O03 runner as same-run qualification checkpoint after baseline load",
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
        if "gates" in payload:
            status, blockers = report.evaluate_status(
                markhand_soak=bool(payload.get("markhandSoak")),
                prerequisites_ok=bool((payload.get("prerequisites") or {}).get("ok")),
                measured=bool((payload.get("metrics") or {}).get("measured")),
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
