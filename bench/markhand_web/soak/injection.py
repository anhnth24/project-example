"""Opt-in-safe failure injection for POC Compose services only."""

from __future__ import annotations

import subprocess
import time
from pathlib import Path
from typing import Any, Callable


class InjectionError(RuntimeError):
    """Unsafe or failed injection attempt."""


# Only these Compose service names may be targeted.
ALLOWED_KILL_SERVICES = ("worker-convert", "worker-index")
ALLOWED_BLIP_SERVICES = ("postgres", "qdrant", "minio")


def resolve_target_container(
    *,
    compose_project: str,
    service: str,
    container_id: str,
    allowed_ids: dict[str, str],
) -> str:
    """Return a container ID only when it matches the expected POC service map.

    ``allowed_ids`` must map service → container id discovered from the Compose
    project label. Arbitrary IDs are refused.
    """
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
    """Map Compose service name → container id for the expected POC project."""
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
                "{{.ID}}\t{{.Label \"com.docker.compose.service\"}}",
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
            ["inspect", "-f", "{{.State.Running}} {{if .State.Health}}{{.State.Health.Status}}{{end}}", container_id],
            runner=runner,
        )
        text = (proc.stdout or "").strip().lower()
        if text.startswith("true") and ("healthy" in text or text == "true"):
            # No healthcheck ⇒ Running=true with empty health is acceptable.
            if "unhealthy" in text:
                time.sleep(1.0)
                continue
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
    """Kill then restart an allowlisted worker; record before/after IDs."""
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
    kill = _docker(["kill", before], runner=runner)
    evidence["killExit"] = kill.returncode
    start = _docker(["start", before], runner=runner)
    evidence["startExit"] = start.returncode
    # Re-discover in case Compose recreates the container.
    mapping = discover_poc_containers(compose_project, runner=runner)
    after = mapping.get(service) or before
    evidence["afterId"] = after
    evidence["recovered"] = wait_healthy(
        after, deadline_seconds=recovery_deadline_seconds, runner=runner
    )
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
    """Stop an allowlisted dependency briefly, then start and wait for health."""
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
    if not evidence["recovered"]:
        raise InjectionError(f"dependency_recovery_timeout:{service}")
    return evidence


def write_injection_evidence(raw_dir: Path, payload: dict[str, Any]) -> Path:
    raw_dir.mkdir(parents=True, exist_ok=True)
    path = raw_dir / f"injection-{payload.get('action', 'unknown')}.json"
    import json

    path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    return path
