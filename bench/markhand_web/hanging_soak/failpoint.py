"""Pause/unpause failpoint for the P1B-R06 hanging-dependency soak.

A *stopped* dependency fails fast: the app gets connection-refused or a
routing error almost immediately. A *paused* dependency (`docker pause`,
which freezes every process in the container's cgroup with the kernel
freezer) is the interesting case the R06 issue text calls out: the kernel
still completes the TCP handshake and queues the connection, but nothing on
the other end ever reads or writes, so a caller with no deadline of its own
blocks forever. That is the failure mode `crates/server/src/services/
readiness.rs` is written to survive (`OUTER_DEADLINE` / `PER_PROBE_DEADLINE`,
see `gate_report.py`), and it is what this harness has to prove against the
real Compose stack.

Arm-before/restore-after-confirmed mirrors the existing P1B-O02 failpoint
protocol in `deploy/scripts/o02-pg-restore-guard.sh`, translated from
stop/start to pause/unpause:

  - Arm the guard *before* calling `docker pause`.
  - Restore (`docker unpause`) fires from a `finally` block *and* from
    `atexit`/SIGTERM/SIGINT handlers, so an aborted run (Ctrl-C, killed
    process, uncaught exception during the sustain window) still leaves
    nothing paused.
  - The arm only clears once `docker unpause` is *confirmed* by
    `docker inspect` (`State.Running=true`, `State.Paused=false`) — never on
    the mere exit code of `docker unpause`.
  - A container that was not already running-and-unpaused before the attempt
    is never touched (mirrors `o02_pg_restore_if_armed`'s
    `precondition_not_running` skip).
"""

from __future__ import annotations

import atexit
import signal
import subprocess
import threading
from dataclasses import dataclass, field
from typing import Any, Callable

Runner = Callable[..., "subprocess.CompletedProcess[str]"]


class FailpointError(RuntimeError):
    """Unsafe or failed pause/unpause attempt. Callers must treat this as fail-closed."""


# Only these Compose services may ever be paused. This is the exact set of
# external dependencies crates/server/src/services/readiness.rs performs a
# network probe against (see DEPENDENCY_PROBES in gate_report.py):
#   postgres        -> ReadinessProbeError::Database    ("ready_database")
#   qdrant          -> ReadinessProbeError::VectorStore  ("ready_vector_store")
#   minio           -> ReadinessProbeError::ObjectStore  ("ready_object_store")
#   mock-embedding / embedding-cpu -> ReadinessProbeError::Embedding ("ready_embedding")
# (service names from `deploy/compose.poc.yml`; mock-embedding is the default
# `COMPOSE_PROFILES=mock` embedding service, embedding-cpu is the aiteamvn
# profile's — see deploy/scripts/poc-health.sh).
ALLOWED_PAUSE_SERVICES = (
    "postgres",
    "qdrant",
    "minio",
    "mock-embedding",
    "embedding-cpu",
)


def _docker(
    args: list[str],
    *,
    runner: Runner | None = None,
    timeout_seconds: float = 30.0,
) -> "subprocess.CompletedProcess[str]":
    run = runner or subprocess.run
    return run(
        ["docker", *args],
        capture_output=True,
        text=True,
        check=False,
        timeout=timeout_seconds,
    )


def discover_poc_containers(
    compose_project: str,
    *,
    runner: Runner | None = None,
    timeout_seconds: float = 15.0,
) -> dict[str, str]:
    """Map compose service name -> container id for the running POC stack.

    Same technique as `bench/markhand_web/soak/injection.py`'s
    `discover_poc_containers` (docker ps filtered by the compose project
    label), duplicated here rather than imported so this R06 harness has no
    runtime dependency on the O05 soak package.
    """
    proc = _docker(
        [
            "ps",
            "-a",
            "--filter",
            f"label=com.docker.compose.project={compose_project}",
            "--format",
            '{{.ID}}\t{{.Label "com.docker.compose.service"}}',
        ],
        runner=runner,
        timeout_seconds=timeout_seconds,
    )
    if proc.returncode != 0:
        raise FailpointError(f"docker_ps_failed:exit_{proc.returncode}")
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


def resolve_target_container(
    *,
    service: str,
    allowed_ids: dict[str, str],
) -> str:
    """Fail closed unless the service is allowlisted and currently discovered."""
    if service not in ALLOWED_PAUSE_SERVICES:
        raise FailpointError(f"service_not_allowlisted:{service}")
    container_id = allowed_ids.get(service)
    if not container_id or len(container_id) < 12:
        raise FailpointError(f"service_not_running:{service}")
    return container_id


def inspect_running_paused(
    cid: str,
    *,
    runner: Runner | None = None,
) -> tuple[bool, bool]:
    """Return (running, paused) for a container id. (False, False) if inspect fails."""
    proc = _docker(
        ["inspect", "-f", "{{.State.Running}} {{.State.Paused}}", cid],
        runner=runner,
        timeout_seconds=10.0,
    )
    if proc.returncode != 0:
        return False, False
    parts = (proc.stdout or "").strip().split()
    running = bool(parts) and parts[0].lower() == "true"
    paused = len(parts) > 1 and parts[1].lower() == "true"
    return running, paused


@dataclass(eq=False)
class PauseGuard:
    """Arm-before-pause / restore-only-after-confirmed-unpause for one container.

    Fail-closed on abort: `restore_if_armed` is idempotent and is registered
    against `atexit` plus SIGTERM/SIGINT as soon as the guard arms, so a
    crashed or killed run still unpauses the dependency it paused.
    """

    service: str
    container_id: str
    runner: Runner | None = None
    armed: bool = False
    pause_confirmed: bool = False
    restore_attempted: bool = False
    restored_confirmed: bool = False
    last_restore_result: dict[str, Any] = field(default_factory=dict)
    _lock: threading.Lock = field(default_factory=threading.Lock)
    _registered: bool = False

    def _register_abort_handlers(self) -> None:
        if self._registered:
            return
        self._registered = True
        atexit.register(self.restore_if_armed)
        for sig in (signal.SIGTERM, signal.SIGINT):
            try:
                previous = signal.getsignal(sig)
            except (ValueError, OSError):
                continue

            def _handler(signum: int, frame: Any, _previous: Any = previous) -> None:
                self.restore_if_armed()
                if callable(_previous):
                    _previous(signum, frame)
                else:
                    raise SystemExit(128 + signum)

            try:
                signal.signal(sig, _handler)
            except (ValueError, OSError):
                pass

    def arm_and_pause(self) -> None:
        """Confirm running+unpaused, arm the restore obligation, then pause."""
        running, paused = inspect_running_paused(self.container_id, runner=self.runner)
        if not running or paused:
            raise FailpointError(
                f"precondition_not_running_unpaused:{self.service}"
            )
        # Arm *before* the pause call itself, exactly like o02_pg_arm_restore
        # arms before `docker stop` — the obligation to restore must exist
        # even if the pause command's own result is never observed (e.g. the
        # process is killed between the syscall and reading its exit code).
        self._register_abort_handlers()
        with self._lock:
            self.armed = True
        proc = _docker(["pause", self.container_id], runner=self.runner)
        if proc.returncode != 0:
            self.restore_if_armed()
            raise FailpointError(
                f"pause_failed:{self.service}:exit_{proc.returncode}"
            )
        _running, paused_now = inspect_running_paused(
            self.container_id, runner=self.runner
        )
        self.pause_confirmed = bool(paused_now)
        if not self.pause_confirmed:
            # The container never actually froze; do not claim a hang test
            # happened, and restore immediately.
            self.restore_if_armed()
            raise FailpointError(f"pause_not_confirmed:{self.service}")

    def restore_if_armed(self) -> dict[str, Any]:
        """Idempotent: unpauses only while armed, disarms only when confirmed."""
        with self._lock:
            if not self.armed:
                result = {"restoreSkipped": True, "armed": False}
                self.last_restore_result = result
                return result
            self.restore_attempted = True
        proc = _docker(["unpause", self.container_id], runner=self.runner)
        running, paused = inspect_running_paused(self.container_id, runner=self.runner)
        confirmed = bool(running and not paused)
        with self._lock:
            self.restored_confirmed = self.restored_confirmed or confirmed
            if confirmed:
                self.armed = False
        result = {
            "restoreSkipped": False,
            "unpauseExitCode": proc.returncode,
            "running": running,
            "paused": paused,
            "restoredConfirmed": confirmed,
        }
        self.last_restore_result = result
        return result
