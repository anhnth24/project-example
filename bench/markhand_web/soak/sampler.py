"""Safe metric sampling: Docker stats, API /metrics, PG connections, container temp."""

from __future__ import annotations

import json
import re
import subprocess
import threading
import time
from pathlib import Path
from typing import Any, Callable
from urllib.error import URLError
from urllib.request import Request, urlopen


# Allowlisted in-container paths for temp byte sampling (no secrets/content).
TEMP_PATH_ALLOWLIST = (
    "/tmp",
    "/var/tmp",
    "/work",
    "/app/tmp",
    "/data/tmp",
)


def _run(
    args: list[str],
    *,
    runner: Callable[..., subprocess.CompletedProcess[str]] | None = None,
) -> subprocess.CompletedProcess[str]:
    run = runner or subprocess.run
    return run(args, capture_output=True, text=True, check=False)


def sample_docker_stats(
    container_ids: dict[str, str],
    *,
    runner: Callable[..., subprocess.CompletedProcess[str]] | None = None,
) -> dict[str, Any]:
    """Return per-service RSS (MiB) from `docker stats --no-stream` (no secrets)."""
    if not container_ids:
        return {"services": {}, "rssMbTotal": None}
    ids = list(container_ids.values())
    proc = _run(
        ["docker", "stats", "--no-stream", "--format", "{{.ID}}\t{{.MemUsage}}", *ids],
        runner=runner,
    )
    by_id: dict[str, float] = {}
    for line in (proc.stdout or "").splitlines():
        if "\t" not in line:
            continue
        cid, mem = line.split("\t", 1)
        match = re.match(r"([0-9.]+)\s*([KMGT]?i?B)", mem.strip(), re.I)
        if not match:
            continue
        value = float(match.group(1))
        unit = match.group(2).lower()
        factor = {
            "b": 1 / (1024 * 1024),
            "kib": 1 / 1024,
            "kb": 1 / 1000,
            "mib": 1.0,
            "mb": 1.0,
            "gib": 1024.0,
            "gb": 1000.0,
        }.get(unit, 1.0)
        by_id[cid.strip()[:12]] = value * factor

    services: dict[str, float] = {}
    total = 0.0
    for service, cid in container_ids.items():
        short = cid[:12]
        mb = None
        for key, val in by_id.items():
            if cid.startswith(key) or key.startswith(short):
                mb = val
                break
        if mb is not None:
            services[service] = round(mb, 3)
            total += mb
    return {"services": services, "rssMbTotal": round(total, 3) if services else None}


def sample_api_metrics(base_url: str, *, timeout: float = 5.0) -> dict[str, Any]:
    """Fetch `/metrics`. Missing series ⇒ None (never fabricate 0)."""
    url = base_url.rstrip("/") + "/metrics"
    req = Request(url, method="GET", headers={"Accept": "text/plain"})
    try:
        with urlopen(req, timeout=timeout) as resp:  # noqa: S310
            text = resp.read().decode("utf-8", errors="replace")
    except (URLError, TimeoutError, OSError) as exc:
        return {
            "ok": False,
            "error": type(exc).__name__,
            "queueDepthMax": None,
            "queueAgeMax": None,
        }

    depth_vals: list[float] = []
    age_vals: list[float] = []
    for line in text.splitlines():
        if line.startswith("#") or not line.strip():
            continue
        if line.startswith("markhand_job_queue_depth"):
            parts = line.split()
            if len(parts) >= 2:
                try:
                    depth_vals.append(float(parts[-1]))
                except ValueError:
                    pass
        if line.startswith("markhand_job_queue_age_seconds"):
            parts = line.split()
            if len(parts) >= 2:
                try:
                    age_vals.append(float(parts[-1]))
                except ValueError:
                    pass
    return {
        "ok": True,
        "queueDepthMax": max(depth_vals) if depth_vals else None,
        "queueAgeMax": max(age_vals) if age_vals else None,
        "queueDepthSeriesPresent": bool(depth_vals),
        "queueAgeSeriesPresent": bool(age_vals),
    }


def sample_pg_connections(
    *,
    compose_project: str,
    postgres_service: str = "postgres",
    container_ids: dict[str, str],
    runner: Callable[..., subprocess.CompletedProcess[str]] | None = None,
) -> dict[str, Any]:
    """Count PG connections via docker exec (no passwords printed)."""
    cid = container_ids.get(postgres_service)
    if not cid:
        return {"ok": False, "connections": None, "error": "postgres_cid_missing"}
    proc = _run(
        [
            "docker",
            "exec",
            cid,
            "psql",
            "-U",
            "markhand",
            "-d",
            "markhand",
            "-tAc",
            "SELECT count(*) FROM pg_stat_activity",
        ],
        runner=runner,
    )
    text = (proc.stdout or "").strip()
    if proc.returncode != 0:
        proc2 = _run(
            [
                "docker",
                "exec",
                cid,
                "psql",
                "-U",
                "postgres",
                "-d",
                "postgres",
                "-tAc",
                "SELECT count(*) FROM pg_stat_activity",
            ],
            runner=runner,
        )
        text = (proc2.stdout or "").strip()
        if proc2.returncode != 0:
            return {
                "ok": False,
                "connections": None,
                "error": "psql_failed",
                "composeProject": compose_project,
            }
    try:
        return {"ok": True, "connections": int(text), "composeProject": compose_project}
    except ValueError:
        return {"ok": False, "connections": None, "error": "parse_failed"}


def sample_container_temp_bytes(
    container_ids: dict[str, str],
    *,
    services: tuple[str, ...] | None = None,
    paths: tuple[str, ...] = TEMP_PATH_ALLOWLIST,
    runner: Callable[..., subprocess.CompletedProcess[str]] | None = None,
) -> dict[str, Any]:
    """Sum allowlisted temp paths inside expected POC containers via `du -sb`."""
    target_services = services or tuple(container_ids.keys())
    total = 0
    observed = 0
    per_service: dict[str, int] = {}
    for service in target_services:
        cid = container_ids.get(service)
        if not cid:
            continue
        service_total = 0
        service_ok = False
        for path in paths:
            if path not in TEMP_PATH_ALLOWLIST:
                continue
            proc = _run(
                ["docker", "exec", cid, "du", "-sb", path],
                runner=runner,
            )
            if proc.returncode != 0:
                continue
            line = (proc.stdout or "").strip().split()
            if not line:
                continue
            try:
                service_total += int(line[0])
                service_ok = True
            except ValueError:
                continue
        if service_ok:
            per_service[service] = service_total
            total += service_total
            observed += 1
    if observed == 0:
        return {"ok": False, "tempBytes": None, "services": per_service}
    return {"ok": True, "tempBytes": total, "services": per_service}


class GrowthTracker:
    """Track RSS / temp start-peak-end growth. Unobserved metrics stay None."""

    def __init__(self) -> None:
        self.rss_start: float | None = None
        self.rss_peak: float | None = None
        self.rss_end: float | None = None
        self.temp_start: int | None = None
        self.temp_peak: int | None = None
        self.temp_end: int | None = None
        self.queue_max: float | None = None
        self.queue_age_max: float | None = None
        self.db_conn_max: int | None = None
        self.queue_observations = 0
        self.db_observations = 0
        self.temp_observations = 0
        self.samples: list[dict[str, Any]] = []

    def observe(
        self,
        *,
        rss_mb: float | None,
        temp_bytes: int | None,
        queue_depth: float | None,
        queue_age: float | None,
        db_conn: int | None,
    ) -> None:
        now = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
        if rss_mb is not None:
            if self.rss_start is None:
                self.rss_start = rss_mb
            self.rss_peak = rss_mb if self.rss_peak is None else max(self.rss_peak, rss_mb)
            self.rss_end = rss_mb
        if temp_bytes is not None:
            self.temp_observations += 1
            if self.temp_start is None:
                self.temp_start = temp_bytes
            self.temp_peak = (
                temp_bytes if self.temp_peak is None else max(self.temp_peak, temp_bytes)
            )
            self.temp_end = temp_bytes
        if queue_depth is not None:
            self.queue_observations += 1
            self.queue_max = (
                float(queue_depth)
                if self.queue_max is None
                else max(self.queue_max, float(queue_depth))
            )
        if queue_age is not None:
            self.queue_age_max = (
                float(queue_age)
                if self.queue_age_max is None
                else max(self.queue_age_max, float(queue_age))
            )
        if db_conn is not None:
            self.db_observations += 1
            self.db_conn_max = (
                int(db_conn) if self.db_conn_max is None else max(self.db_conn_max, int(db_conn))
            )
        self.samples.append(
            {
                "at": now,
                "rssMb": rss_mb,
                "tempBytes": temp_bytes,
                "queueDepth": queue_depth,
                "queueAge": queue_age,
                "dbConnections": db_conn,
            }
        )

    def summary(self) -> dict[str, Any]:
        rss_growth = None
        if self.rss_start is not None and self.rss_peak is not None:
            rss_growth = max(0.0, self.rss_peak - self.rss_start)
        temp_growth = None
        if self.temp_start is not None and self.temp_peak is not None:
            temp_growth = max(0, self.temp_peak - self.temp_start)
        return {
            "rssMb": {
                "start": self.rss_start,
                "peak": self.rss_peak,
                "end": self.rss_end,
                "growth": rss_growth,
            },
            "tempBytes": {
                "start": self.temp_start,
                "peak": self.temp_peak,
                "end": self.temp_end,
                "growth": temp_growth,
            },
            "queueDepthMax": self.queue_max if self.queue_observations else None,
            "queueAgeMaxSeconds": self.queue_age_max if self.queue_observations else None,
            "dbConnectionsMax": self.db_conn_max if self.db_observations else None,
            "queueObservations": self.queue_observations,
            "dbObservations": self.db_observations,
            "tempObservations": self.temp_observations,
            "sampleCount": len(self.samples),
        }

    def write_raw(self, raw_dir: Path) -> None:
        raw_dir.mkdir(parents=True, exist_ok=True)
        (raw_dir / "growth-samples.json").write_text(
            json.dumps({"samples": self.samples, "summary": self.summary()}, indent=2) + "\n",
            encoding="utf-8",
        )


class BackgroundSampler:
    """Periodic sampler thread so the workload scheduler stays non-blocking."""

    def __init__(
        self,
        *,
        interval_seconds: float,
        sample_fn: Callable[[], None],
    ) -> None:
        self.interval_seconds = max(0.5, float(interval_seconds))
        self._sample_fn = sample_fn
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None
        self.errors: list[str] = []

    def start(self) -> None:
        self._thread = threading.Thread(target=self._loop, name="o05-sampler", daemon=True)
        self._thread.start()

    def stop(self, timeout: float = 10.0) -> None:
        self._stop.set()
        if self._thread is not None:
            self._thread.join(timeout=timeout)

    def _loop(self) -> None:
        while not self._stop.is_set():
            try:
                self._sample_fn()
            except Exception as exc:  # noqa: BLE001 — recorded; does not kill soak
                self.errors.append(f"{type(exc).__name__}:{exc}")
            self._stop.wait(self.interval_seconds)
