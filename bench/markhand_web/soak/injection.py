"""Opt-in-safe failure injection for POC Compose services only.

Injection operations run on a dedicated executor so the workload scheduler is
never blocked by stop/sleep/recovery. Windows are recorded thread-safely.
"""

from __future__ import annotations

import concurrent.futures
import json
import subprocess
import threading
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable


class InjectionError(RuntimeError):
    """Unsafe or failed injection attempt."""


ALLOWED_KILL_SERVICES = ("worker-convert", "worker-index")
ALLOWED_BLIP_SERVICES = ("postgres", "qdrant", "minio")


def resolve_target_container(
    *,
    compose_project: str,
    service: str,
    container_id: str,
    allowed_ids: dict[str, str],
) -> str:
    if not compose_project or compose_project != compose_project.strip():
        raise InjectionError("compose_project required")
    expected = allowed_ids.get(service)
    if not expected:
        raise InjectionError(f"service_not_allowlisted:{service}")
    if not container_id or container_id != expected:
        raise InjectionError(f"container_id_mismatch:{service}")
    if len(container_id) < 12:
        raise InjectionError("container_id_too_short")
    return container_id


def discover_poc_containers(
    compose_project: str,
    *,
    runner: Callable[..., subprocess.CompletedProcess[str]] | None = None,
) -> dict[str, str]:
    run = runner or subprocess.run
    try:
        proc = run(
            [
                "docker",
                "ps",
                "-a",
                "--filter",
                f"label=com.docker.compose.project={compose_project}",
                "--format",
                '{{.ID}}\t{{.Label "com.docker.compose.service"}}',
            ],
            capture_output=True,
            text=True,
            check=False,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        raise InjectionError(f"docker_ps_failed:{exc}") from exc
    mapping: dict[str, str] = {}
    for line in (proc.stdout or "").splitlines():
        if "\t" not in line:
            continue
        cid, service = line.split("\t", 1)
        service = service.strip()
        cid = cid.strip()
        if service and cid:
            mapping[service] = cid
    return mapping


def _docker(
    args: list[str],
    *,
    runner: Callable[..., subprocess.CompletedProcess[str]] | None = None,
) -> subprocess.CompletedProcess[str]:
    run = runner or subprocess.run
    return run(["docker", *args], capture_output=True, text=True, check=False)


def wait_healthy(
    container_id: str,
    *,
    deadline_seconds: float = 120.0,
    runner: Callable[..., subprocess.CompletedProcess[str]] | None = None,
) -> bool:
    deadline = time.monotonic() + deadline_seconds
    while time.monotonic() < deadline:
        proc = _docker(
            [
                "inspect",
                "-f",
                "{{.State.Running}} {{if .State.Health}}{{.State.Health.Status}}{{end}}",
                container_id,
            ],
            runner=runner,
        )
        text = (proc.stdout or "").strip().lower()
        if text.startswith("true") and "unhealthy" not in text:
            return True
        time.sleep(1.0)
    return False


def kill_and_restart_worker(
    *,
    compose_project: str,
    service: str,
    allowed_ids: dict[str, str],
    recovery_deadline_seconds: float = 120.0,
    runner: Callable[..., subprocess.CompletedProcess[str]] | None = None,
) -> dict[str, Any]:
    if service not in ALLOWED_KILL_SERVICES:
        raise InjectionError(f"kill_service_not_allowed:{service}")
    before = resolve_target_container(
        compose_project=compose_project,
        service=service,
        container_id=allowed_ids[service],
        allowed_ids=allowed_ids,
    )
    evidence: dict[str, Any] = {
        "action": "kill_worker",
        "composeProject": compose_project,
        "service": service,
        "beforeId": before,
        "afterId": None,
        "recovered": False,
        "recoveryDeadlineSeconds": recovery_deadline_seconds,
    }
    started = time.monotonic()
    evidence["windowStartMono"] = started
    kill = _docker(["kill", before], runner=runner)
    evidence["killExit"] = kill.returncode
    start = _docker(["start", before], runner=runner)
    evidence["startExit"] = start.returncode
    mapping = discover_poc_containers(compose_project, runner=runner)
    after = mapping.get(service) or before
    evidence["afterId"] = after
    evidence["recovered"] = wait_healthy(
        after, deadline_seconds=recovery_deadline_seconds, runner=runner
    )
    evidence["windowEndMono"] = time.monotonic()
    evidence["recoveryLatencySeconds"] = round(evidence["windowEndMono"] - started, 3)
    if not evidence["recovered"]:
        raise InjectionError(f"worker_recovery_timeout:{service}")
    return evidence


def dependency_blip(
    *,
    compose_project: str,
    service: str,
    allowed_ids: dict[str, str],
    blip_seconds: int,
    recovery_deadline_seconds: float = 180.0,
    runner: Callable[..., subprocess.CompletedProcess[str]] | None = None,
    sleeper: Callable[[float], None] | None = None,
) -> dict[str, Any]:
    if service not in ALLOWED_BLIP_SERVICES:
        raise InjectionError(f"blip_service_not_allowed:{service}")
    sleep = sleeper or time.sleep
    before = resolve_target_container(
        compose_project=compose_project,
        service=service,
        container_id=allowed_ids[service],
        allowed_ids=allowed_ids,
    )
    evidence: dict[str, Any] = {
        "action": "dependency_blip",
        "composeProject": compose_project,
        "service": service,
        "beforeId": before,
        "afterId": None,
        "blipSeconds": blip_seconds,
        "recovered": False,
        "recoveryDeadlineSeconds": recovery_deadline_seconds,
    }
    started = time.monotonic()
    evidence["windowStartMono"] = started
    stop = _docker(["stop", before], runner=runner)
    evidence["stopExit"] = stop.returncode
    sleep(max(0, int(blip_seconds)))
    start = _docker(["start", before], runner=runner)
    evidence["startExit"] = start.returncode
    mapping = discover_poc_containers(compose_project, runner=runner)
    after = mapping.get(service) or before
    evidence["afterId"] = after
    evidence["recovered"] = wait_healthy(
        after, deadline_seconds=recovery_deadline_seconds, runner=runner
    )
    evidence["windowEndMono"] = time.monotonic()
    evidence["recoveryLatencySeconds"] = round(evidence["windowEndMono"] - started, 3)
    if not evidence["recovered"]:
        raise InjectionError(f"dependency_recovery_timeout:{service}")
    return evidence


def write_injection_evidence(raw_dir: Path, payload: dict[str, Any]) -> Path:
    raw_dir.mkdir(parents=True, exist_ok=True)
    stamp = payload.get("eventId") or payload.get("action", "unknown")
    path = raw_dir / f"injection-{stamp}.json"
    path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    return path


@dataclass
class InjectionPlan:
    """Tracks every scheduled injection; requires expected==observed and all recovered."""

    expected: list[dict[str, Any]] = field(default_factory=list)
    events: list[dict[str, Any]] = field(default_factory=list)
    windows: list[tuple[float, float]] = field(default_factory=list)
    errors: list[str] = field(default_factory=list)
    lock: threading.Lock = field(default_factory=threading.Lock)
    _pool: concurrent.futures.ThreadPoolExecutor | None = None
    _futures: list[concurrent.futures.Future[dict[str, Any]]] = field(default_factory=list)
    workload_start_mono: float = 0.0

    def start_pool(self, max_workers: int = 2) -> None:
        self._pool = concurrent.futures.ThreadPoolExecutor(
            max_workers=max_workers, thread_name_prefix="o05-inject"
        )

    def schedule(
        self,
        *,
        kind: str,
        scheduled_at: float,
        fn: Callable[[], dict[str, Any]],
    ) -> None:
        if self._pool is None:
            raise InjectionError("injection_pool_not_started")
        event_id = f"{kind}-{len(self.expected)}"
        with self.lock:
            self.expected.append(
                {"eventId": event_id, "kind": kind, "scheduledAtSeconds": scheduled_at}
            )

        def _run() -> dict[str, Any]:
            wall_start = time.monotonic()
            # Window relative to workload start for error classification.
            rel_start = wall_start - self.workload_start_mono
            try:
                evidence = fn()
            except Exception as exc:  # noqa: BLE001
                with self.lock:
                    self.errors.append(f"{event_id}:{type(exc).__name__}:{exc}")
                raise
            evidence = dict(evidence)
            evidence["eventId"] = event_id
            evidence["kind"] = kind
            evidence["scheduledAtSeconds"] = scheduled_at
            rel_end = time.monotonic() - self.workload_start_mono
            evidence["windowStartRel"] = rel_start
            evidence["windowEndRel"] = rel_end
            with self.lock:
                self.events.append(evidence)
                self.windows.append((rel_start, rel_end))
            return evidence

        assert self._pool is not None
        self._futures.append(self._pool.submit(_run))

    def in_window(self, rel_offset: float) -> bool:
        with self.lock:
            windows = list(self.windows)
        for start, end in windows:
            if start <= rel_offset <= end:
                return True
        return False

    def join(self, timeout: float | None = None) -> dict[str, Any]:
        errors: list[str] = []
        for fut in self._futures:
            try:
                fut.result(timeout=timeout)
            except Exception as exc:  # noqa: BLE001
                errors.append(f"{type(exc).__name__}:{exc}")
        if self._pool is not None:
            self._pool.shutdown(wait=True, cancel_futures=False)
            self._pool = None
        with self.lock:
            expected_n = len(self.expected)
            observed_n = len(self.events)
            recovered = [bool(e.get("recovered")) for e in self.events]
            kills_expected = sum(1 for e in self.expected if e["kind"] == "kill_worker")
            kills_observed = sum(1 for e in self.events if e.get("action") == "kill_worker")
            blips_expected = sum(1 for e in self.expected if e["kind"] == "dependency_blip")
            blips_observed = sum(1 for e in self.events if e.get("action") == "dependency_blip")
            all_recovered = bool(recovered) and all(recovered) and not errors and not self.errors
            counts_ok = (
                expected_n == observed_n
                and kills_expected == kills_observed
                and blips_expected == blips_observed
            )
            ok = counts_ok and all_recovered and expected_n > 0
            summary = {
                "ok": ok,
                "expected": expected_n,
                "observed": observed_n,
                "killsExpected": kills_expected,
                "killsObserved": kills_observed,
                "blipsExpected": blips_expected,
                "blipsObserved": blips_observed,
                "allRecovered": all_recovered,
                "countsMatch": counts_ok,
                "errors": list(self.errors) + errors,
                "events": list(self.events),
                "windows": list(self.windows),
            }
        if not ok:
            raise InjectionError(
                "injection_incomplete:"
                + json.dumps(
                    {
                        "expected": expected_n,
                        "observed": observed_n,
                        "killsExpected": kills_expected,
                        "killsObserved": kills_observed,
                        "errors": summary["errors"],
                    }
                )
            )
        return summary
