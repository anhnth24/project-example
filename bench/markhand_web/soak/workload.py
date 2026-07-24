"""Timed concurrent mixed-load against the configurable POC API."""

from __future__ import annotations

import concurrent.futures
import json
import mimetypes
import os
import threading
import time
import uuid
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

from mathutil import percentile, schedule_event_times


ROOT = Path(__file__).resolve().parents[3]
GOLDEN_DOCS = ROOT / "bench/markhand_web/golden/documents"

FORMAT_FIXTURES: dict[str, str] = {
    "pdf": "gold-001.pdf",
    "docx": "gold-006.docx",
    "pptx": "gold-009.pptx",
    "xlsx": "gold-011.xlsx",
    "csv": "gold-014.csv",
    "html": "gold-017.html",
    "txt": "gold-025.txt",
    "png": "gold-020.png",
}


@dataclass
class RequestStats:
    lock: threading.Lock = field(default_factory=threading.Lock)
    counts: dict[str, int] = field(
        default_factory=lambda: {
            "ingest": 0,
            "query": 0,
            "delete": 0,
            "reconcile": 0,
            "errors": 0,
        }
    )
    query_latencies_ms: list[float] = field(default_factory=list)
    ingest_ok: int = 0
    document_ids: list[str] = field(default_factory=list)
    deleted_ids: list[str] = field(default_factory=list)

    def add(self, kind: str, *, ok: bool, latency_ms: float | None = None, doc_id: str | None = None) -> None:
        with self.lock:
            self.counts[kind] = self.counts.get(kind, 0) + 1
            if not ok:
                self.counts["errors"] += 1
            if kind == "query" and latency_ms is not None:
                self.query_latencies_ms.append(latency_ms)
            if kind == "ingest" and ok:
                self.ingest_ok += 1
                if doc_id:
                    self.document_ids.append(doc_id)
            if kind == "delete" and ok and doc_id:
                self.deleted_ids.append(doc_id)


class ApiClient:
    def __init__(
        self,
        base_url: str,
        *,
        token: str | None,
        collection_id: str,
        timeout_seconds: float = 30.0,
        max_in_flight: int = 32,
    ) -> None:
        self.base_url = base_url.rstrip("/")
        self.token = token
        self.collection_id = collection_id
        self.timeout_seconds = timeout_seconds
        self._sema = threading.BoundedSemaphore(max_in_flight)

    def _headers(self, content_type: str | None = "application/json") -> dict[str, str]:
        headers: dict[str, str] = {}
        if content_type:
            headers["Content-Type"] = content_type
        if self.token:
            headers["Authorization"] = f"Bearer {self.token}"
        return headers

    def request(
        self,
        method: str,
        path: str,
        *,
        body: bytes | None = None,
        headers: dict[str, str] | None = None,
    ) -> tuple[int, bytes, float]:
        url = self.base_url + path
        req = Request(url, data=body, method=method, headers=headers or self._headers())
        started = time.perf_counter()
        acquired = self._sema.acquire(timeout=self.timeout_seconds)
        if not acquired:
            return 0, b"backpressure", (time.perf_counter() - started) * 1000.0
        try:
            with urlopen(req, timeout=self.timeout_seconds) as resp:  # noqa: S310
                data = resp.read()
                status = int(getattr(resp, "status", 200))
        except HTTPError as exc:
            data = exc.read() if hasattr(exc, "read") else b""
            status = int(exc.code)
        except (URLError, TimeoutError, OSError):
            data = b""
            status = 0
        finally:
            self._sema.release()
        latency = (time.perf_counter() - started) * 1000.0
        return status, data, latency


def login(base_url: str, email: str, password: str, *, timeout: float = 15.0) -> str:
    body = json.dumps({"email": email, "password": password}).encode("utf-8")
    req = Request(
        base_url.rstrip("/") + "/api/v1/auth/login",
        data=body,
        method="POST",
        headers={"Content-Type": "application/json"},
    )
    with urlopen(req, timeout=timeout) as resp:  # noqa: S310
        payload = json.loads(resp.read().decode("utf-8"))
    token = payload.get("accessToken") or payload.get("access_token")
    if not isinstance(token, str) or not token:
        raise RuntimeError("login_missing_access_token")
    return token


def fixture_path(fmt: str) -> Path:
    name = FORMAT_FIXTURES.get(fmt.lower())
    if not name:
        raise KeyError(f"unsupported_format:{fmt}")
    path = GOLDEN_DOCS / name
    if not path.is_file():
        raise FileNotFoundError(path)
    return path


def _multipart(file_path: Path, collection_id: str) -> tuple[bytes, str]:
    boundary = f"----markhandsoak{uuid.uuid4().hex}"
    content_type = mimetypes.guess_type(file_path.name)[0] or "application/octet-stream"
    file_bytes = file_path.read_bytes()
    chunks = [
        f"--{boundary}\r\n".encode(),
        (
            f'Content-Disposition: form-data; name="file"; filename="{file_path.name}"\r\n'
            f"Content-Type: {content_type}\r\n\r\n"
        ).encode(),
        file_bytes,
        b"\r\n",
        f"--{boundary}\r\n".encode(),
        (
            f'Content-Disposition: form-data; name="collectionId"\r\n\r\n'
            f"{collection_id}\r\n"
        ).encode(),
        f"--{boundary}--\r\n".encode(),
    ]
    return b"".join(chunks), f"multipart/form-data; boundary={boundary}"


def do_ingest(client: ApiClient, fmt: str, stats: RequestStats) -> None:
    path = fixture_path(fmt)
    body, content_type = _multipart(path, client.collection_id)
    status, data, _latency = client.request(
        "POST",
        "/api/v1/uploads",
        body=body,
        headers=client._headers(content_type),
    )
    doc_id = None
    ok = status in {200, 201}
    if ok:
        try:
            payload = json.loads(data.decode("utf-8"))
            doc_id = payload.get("documentId")
        except (UnicodeDecodeError, json.JSONDecodeError):
            ok = False
    stats.add("ingest", ok=ok, doc_id=doc_id if isinstance(doc_id, str) else None)


def do_query(client: ApiClient, mode: str, stats: RequestStats) -> None:
    body_obj: dict[str, Any] = {
        "query": "markhand soak synthetic query",
        "mode": mode,
        "limit": 5,
        "collectionIds": [client.collection_id],
    }
    if mode == "as_of":
        body_obj["asOf"] = "2026-01-01T00:00:00Z"
    if mode == "compare":
        # Compare without specific versions still exercises the route; API may 400.
        body_obj["documentId"] = str(uuid.uuid4())
    body = json.dumps(body_obj).encode("utf-8")
    status, _data, latency = client.request("POST", "/api/v1/search", body=body)
    ok = status in {200, 400}  # 400 for incomplete compare still counts as timed query
    stats.add("query", ok=ok, latency_ms=latency)


def do_delete(client: ApiClient, stats: RequestStats) -> None:
    with stats.lock:
        if not stats.document_ids:
            doc_id = None
        else:
            doc_id = stats.document_ids.pop(0)
    if not doc_id:
        stats.add("delete", ok=False)
        return
    status, _data, _latency = client.request("DELETE", f"/api/v1/documents/{doc_id}")
    stats.add("delete", ok=status in {200, 204}, doc_id=doc_id)


def do_reconcile(
    *,
    compose_project: str,
    document_id: str | None,
    stats: RequestStats,
    runner: Callable[..., Any] | None = None,
) -> None:
    """Invoke the approved Compose reconcile oneshot (no arbitrary containers)."""
    import subprocess

    run = runner or subprocess.run
    env = os.environ.copy()
    env["MARKHAND_RECONCILE_MODE"] = "dry-run"
    if document_id:
        env["MARKHAND_RECONCILE_DOCUMENT_ID"] = document_id
    compose_file = ROOT / "deploy/compose.poc.yml"
    cmd = [
        "docker",
        "compose",
        "-p",
        compose_project,
        "-f",
        str(compose_file),
        "--profile",
        "reconcile-oneshot",
        "run",
        "--rm",
        "--no-deps",
        "worker-reconcile-oneshot",
    ]
    try:
        proc = run(cmd, capture_output=True, text=True, check=False, env=env, timeout=90)
        ok = proc.returncode == 0
    except (OSError, subprocess.SubprocessError):
        ok = False
    stats.add("reconcile", ok=ok)


def run_mixed_load(
    *,
    client: ApiClient,
    profile: dict[str, Any],
    duration_seconds: int,
    compose_project: str,
    on_tick: Callable[[float], None] | None = None,
    enable_reconcile: bool = True,
) -> RequestStats:
    """Execute scheduled ingest/query/delete/reconcile for ``duration_seconds``."""
    stats = RequestStats()
    actors = profile["actors"]
    formats = list(actors["ingest"]["formats"])
    modes = list(actors["query"]["modes"])
    ingest_times = schedule_event_times(
        rps=float(actors["ingest"]["rps"]), duration_seconds=float(duration_seconds), seed=1
    )
    query_times = schedule_event_times(
        rps=float(actors["query"]["rps"]), duration_seconds=float(duration_seconds), seed=2
    )
    delete_times = schedule_event_times(
        rps=float(actors["delete"]["rps"]), duration_seconds=float(duration_seconds), seed=3
    )
    interval = int(actors["reconcile"].get("intervalSeconds") or 300)
    reconcile_times = (
        list(range(interval, duration_seconds, interval)) if enable_reconcile and interval > 0 else []
    )

    events: list[tuple[float, str, Any]] = []
    for i, t in enumerate(ingest_times):
        events.append((t, "ingest", formats[i % len(formats)]))
    for i, t in enumerate(query_times):
        events.append((t, "query", modes[i % len(modes)]))
    for t in delete_times:
        events.append((t, "delete", None))
    for t in reconcile_times:
        events.append((float(t), "reconcile", None))
    events.sort(key=lambda row: row[0])

    start = time.monotonic()
    idx = 0
    with concurrent.futures.ThreadPoolExecutor(max_workers=16) as pool:
        futures: list[concurrent.futures.Future[None]] = []
        while True:
            elapsed = time.monotonic() - start
            if elapsed >= duration_seconds:
                break
            if on_tick is not None:
                on_tick(elapsed)
            while idx < len(events) and events[idx][0] <= elapsed:
                _t, kind, arg = events[idx]
                idx += 1
                if kind == "ingest":
                    futures.append(pool.submit(do_ingest, client, str(arg), stats))
                elif kind == "query":
                    futures.append(pool.submit(do_query, client, str(arg), stats))
                elif kind == "delete":
                    futures.append(pool.submit(do_delete, client, stats))
                elif kind == "reconcile":
                    doc = stats.document_ids[-1] if stats.document_ids else None
                    futures.append(
                        pool.submit(
                            do_reconcile,
                            compose_project=compose_project,
                            document_id=doc,
                            stats=stats,
                        )
                    )
            # Bound in-flight futures list
            futures = [f for f in futures if not f.done()]
            time.sleep(0.05)
        concurrent.futures.wait(futures, timeout=client.timeout_seconds + 5)

    return stats


def metrics_from_stats(stats: RequestStats, duration_seconds: int) -> dict[str, Any]:
    hours = max(duration_seconds, 1) / 3600.0
    return {
        "requestCounts": dict(stats.counts),
        "requestErrors": stats.counts.get("errors", 0),
        "queryP50Ms": percentile(stats.query_latencies_ms, 50),
        "queryP95Ms": percentile(stats.query_latencies_ms, 95),
        "queryP99Ms": percentile(stats.query_latencies_ms, 99),
        "ingestDocsPerHour": stats.ingest_ok / hours,
        "ingestOk": stats.ingest_ok,
        "deletedCount": len(stats.deleted_ids),
        "durationSeconds": duration_seconds,
    }


def post_restore_retrieval_check(
    client: ApiClient,
    deleted_ids: list[str],
) -> dict[str, Any]:
    """Ensure deleted document ids are not returned as authorized hits."""
    if not deleted_ids:
        return {"passed": False, "reason": "no_deleted_ids"}
    body = json.dumps(
        {
            "query": "markhand soak post-restore",
            "mode": "current",
            "limit": 20,
            "collectionIds": [client.collection_id],
        }
    ).encode("utf-8")
    status, data, _latency = client.request("POST", "/api/v1/search", body=body)
    if status != 200:
        return {"passed": False, "reason": f"search_status_{status}"}
    try:
        payload = json.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError):
        return {"passed": False, "reason": "invalid_json"}
    hits = payload.get("hits") or []
    hit_docs = set()
    for hit in hits:
        if isinstance(hit, dict):
            for key in ("documentId", "document_id"):
                if hit.get(key):
                    hit_docs.add(str(hit.get(key)))
    leaked = [d for d in deleted_ids if d in hit_docs]
    return {"passed": not leaked, "leakedDeletedIds": len(leaked), "hitCount": len(hits)}
