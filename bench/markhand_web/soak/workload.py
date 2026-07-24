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

from fixtures import fixture_path, preflight_fixtures
from mathutil import percentile, schedule_event_times


ROOT = Path(__file__).resolve().parents[3]

# Completeness: successful completions must reach this fraction of scheduled
# events outside the injection-window allowance.
COMPLETENESS_RATIO = 0.95


@dataclass
class DocVersion:
    document_id: str
    version_id: str
    published: bool = False


@dataclass
class RequestStats:
    lock: threading.Lock = field(default_factory=threading.Lock)
    scheduled: dict[str, int] = field(
        default_factory=lambda: {"ingest": 0, "query": 0, "delete": 0, "reconcile": 0}
    )
    submitted: dict[str, int] = field(
        default_factory=lambda: {"ingest": 0, "query": 0, "delete": 0, "reconcile": 0}
    )
    completed: dict[str, int] = field(
        default_factory=lambda: {"ingest": 0, "query": 0, "delete": 0, "reconcile": 0}
    )
    success: dict[str, int] = field(
        default_factory=lambda: {"ingest": 0, "query": 0, "delete": 0, "reconcile": 0}
    )
    errors: int = 0
    errors_outside_injection: int = 0
    errors_in_injection: int = 0
    query_success_latencies_ms: list[float] = field(default_factory=list)
    query_success_by_mode: dict[str, int] = field(default_factory=dict)
    query_attempts_by_mode: dict[str, int] = field(default_factory=dict)
    ingest_ok: int = 0
    document_ids: list[str] = field(default_factory=list)
    deleted_ids: list[str] = field(default_factory=list)
    retained_ids: list[str] = field(default_factory=list)
    versions: dict[str, list[DocVersion]] = field(default_factory=dict)
    not_ready: list[str] = field(default_factory=list)
    exceptions: list[str] = field(default_factory=list)
    injection_windows: list[tuple[float, float]] = field(default_factory=list)

    def mark_scheduled(self, kind: str, n: int = 1) -> None:
        with self.lock:
            self.scheduled[kind] = self.scheduled.get(kind, 0) + n

    def mark_submitted(self, kind: str) -> None:
        with self.lock:
            self.submitted[kind] = self.submitted.get(kind, 0) + 1

    def in_injection_window(self, monotonic_offset: float) -> bool:
        with self.lock:
            windows = list(self.injection_windows)
        for start, end in windows:
            if start <= monotonic_offset <= end:
                return True
        return False

    def add_injection_window(self, start: float, end: float) -> None:
        with self.lock:
            self.injection_windows.append((start, end))

    def add(
        self,
        kind: str,
        *,
        ok: bool,
        latency_ms: float | None = None,
        doc_id: str | None = None,
        mode: str | None = None,
        in_injection: bool = False,
        not_ready_reason: str | None = None,
    ) -> None:
        with self.lock:
            self.completed[kind] = self.completed.get(kind, 0) + 1
            if not_ready_reason:
                self.not_ready.append(not_ready_reason)
            if ok:
                self.success[kind] = self.success.get(kind, 0) + 1
            else:
                self.errors += 1
                if in_injection:
                    self.errors_in_injection += 1
                else:
                    self.errors_outside_injection += 1
            if kind == "query":
                if mode:
                    self.query_attempts_by_mode[mode] = (
                        self.query_attempts_by_mode.get(mode, 0) + 1
                    )
                if ok and latency_ms is not None:
                    self.query_success_latencies_ms.append(latency_ms)
                    if mode:
                        self.query_success_by_mode[mode] = (
                            self.query_success_by_mode.get(mode, 0) + 1
                        )
            if kind == "ingest" and ok:
                self.ingest_ok += 1
                if doc_id:
                    self.document_ids.append(doc_id)
            if kind == "delete" and ok and doc_id:
                self.deleted_ids.append(doc_id)

    def record_version(self, document_id: str, version_id: str, *, published: bool = False) -> None:
        with self.lock:
            self.versions.setdefault(document_id, []).append(
                DocVersion(document_id, version_id, published=published)
            )

    def compare_pair(self) -> tuple[str, str, str] | None:
        """Return (documentId, versionA, versionB) when two versions exist."""
        with self.lock:
            for doc_id, vers in self.versions.items():
                if len(vers) >= 2:
                    return doc_id, vers[0].version_id, vers[1].version_id
        return None

    def as_of_doc(self) -> str | None:
        with self.lock:
            for doc_id, vers in self.versions.items():
                if vers:
                    return doc_id
            return self.document_ids[0] if self.document_ids else None


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


def _http_success(status: int) -> bool:
    return 200 <= status < 300


def do_ingest(
    client: ApiClient,
    fmt: str,
    stats: RequestStats,
    *,
    start_mono: float,
) -> None:
    path = fixture_path(fmt)
    body, content_type = _multipart(path, client.collection_id)
    status, data, _latency = client.request(
        "POST",
        "/api/v1/uploads",
        body=body,
        headers=client._headers(content_type),
    )
    doc_id = None
    version_id = None
    ok = _http_success(status)
    if ok:
        try:
            payload = json.loads(data.decode("utf-8"))
            doc_id = payload.get("documentId")
            version_id = payload.get("versionId")
        except (UnicodeDecodeError, json.JSONDecodeError):
            ok = False
    if ok and isinstance(doc_id, str) and isinstance(version_id, str):
        stats.record_version(doc_id, version_id, published=False)
        # Seed a second synthetic version id slot when API returns only one —
        # compare still needs a real pair; mark not-ready until second upload
        # of same doc or publish+new version arrives. For soak we re-upload
        # same format later; pair forms when len>=2 for a doc.
    in_inj = stats.in_injection_window(time.monotonic() - start_mono)
    stats.add(
        "ingest",
        ok=ok,
        doc_id=doc_id if isinstance(doc_id, str) else None,
        in_injection=in_inj,
    )


def do_query(
    client: ApiClient,
    mode: str,
    stats: RequestStats,
    *,
    start_mono: float,
) -> None:
    body_obj: dict[str, Any] = {
        "query": "markhand soak synthetic query",
        "mode": mode,
        "limit": 5,
        "collectionIds": [client.collection_id],
    }
    not_ready = None
    if mode == "as_of":
        doc = stats.as_of_doc()
        if not doc:
            not_ready = "as_of_no_document"
        else:
            body_obj["asOf"] = "2026-07-01T00:00:00Z"
            body_obj["documentId"] = doc
    elif mode == "compare":
        pair = stats.compare_pair()
        if not pair:
            not_ready = "compare_no_version_pair"
        else:
            doc_id, ver_a, ver_b = pair
            body_obj["documentId"] = doc_id
            body_obj["versionA"] = ver_a
            body_obj["versionB"] = ver_b
    elif mode == "current":
        pass
    else:
        not_ready = f"unsupported_mode:{mode}"

    in_inj = stats.in_injection_window(time.monotonic() - start_mono)
    if not_ready:
        stats.add(
            "query",
            ok=False,
            mode=mode,
            in_injection=in_inj,
            not_ready_reason=not_ready,
        )
        return

    body = json.dumps(body_obj).encode("utf-8")
    status, _data, latency = client.request("POST", "/api/v1/search", body=body)
    ok = _http_success(status)
    # Only successful (2xx) queries contribute latency samples.
    stats.add(
        "query",
        ok=ok,
        latency_ms=latency if ok else None,
        mode=mode,
        in_injection=in_inj,
    )


def do_delete(client: ApiClient, stats: RequestStats, *, start_mono: float) -> None:
    with stats.lock:
        if not stats.document_ids:
            doc_id = None
        else:
            # Keep at least one retained doc for post-restore authorized retrieval.
            if len(stats.document_ids) <= 1 and not stats.retained_ids:
                stats.retained_ids.append(stats.document_ids[0])
                doc_id = None
            else:
                doc_id = stats.document_ids.pop(0)
                if not stats.retained_ids and stats.document_ids:
                    stats.retained_ids.append(stats.document_ids[0])
    in_inj = stats.in_injection_window(time.monotonic() - start_mono)
    if not doc_id:
        stats.add("delete", ok=False, in_injection=in_inj, not_ready_reason="delete_no_doc")
        return
    status, _data, _latency = client.request("DELETE", f"/api/v1/documents/{doc_id}")
    stats.add("delete", ok=_http_success(status), doc_id=doc_id, in_injection=in_inj)


def do_reconcile(
    *,
    compose_project: str,
    document_id: str | None,
    stats: RequestStats,
    start_mono: float,
    runner: Callable[..., Any] | None = None,
) -> None:
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
    in_inj = stats.in_injection_window(time.monotonic() - start_mono)
    try:
        proc = run(cmd, capture_output=True, text=True, check=False, env=env, timeout=90)
        ok = proc.returncode == 0
    except (OSError, subprocess.SubprocessError):
        ok = False
    stats.add("reconcile", ok=ok, in_injection=in_inj)


def expected_scheduled_counts(profile: dict[str, Any], duration_seconds: int) -> dict[str, int]:
    actors = profile["actors"]
    ingest = len(
        schedule_event_times(
            rps=float(actors["ingest"]["rps"]),
            duration_seconds=float(duration_seconds),
            seed=1,
        )
    )
    query = len(
        schedule_event_times(
            rps=float(actors["query"]["rps"]),
            duration_seconds=float(duration_seconds),
            seed=2,
        )
    )
    delete = len(
        schedule_event_times(
            rps=float(actors["delete"]["rps"]),
            duration_seconds=float(duration_seconds),
            seed=3,
        )
    )
    interval = int(actors["reconcile"].get("intervalSeconds") or 300)
    reconcile = len(list(range(interval, duration_seconds, interval))) if interval > 0 else 0
    return {"ingest": ingest, "query": query, "delete": delete, "reconcile": reconcile}


def run_mixed_load(
    *,
    client: ApiClient,
    profile: dict[str, Any],
    duration_seconds: int,
    compose_project: str,
    enable_reconcile: bool = True,
    injection_callback: Callable[[float, str, RequestStats], None] | None = None,
    injection_schedule: list[tuple[float, str]] | None = None,
    max_workers: int = 16,
) -> RequestStats:
    """Execute scheduled ingest/query/delete/reconcile for ``duration_seconds``.

    Sampler must run out-of-band; this loop only schedules work on monotonic time.
    Futures are drained via ``result()`` so worker exceptions propagate.
    """
    formats = list(profile["actors"]["ingest"]["formats"])
    preflight_fixtures(formats)

    stats = RequestStats()
    actors = profile["actors"]
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

    stats.mark_scheduled("ingest", len(ingest_times))
    stats.mark_scheduled("query", len(query_times))
    stats.mark_scheduled("delete", len(delete_times))
    stats.mark_scheduled("reconcile", len(reconcile_times))

    events: list[tuple[float, str, Any]] = []
    for i, t in enumerate(ingest_times):
        events.append((t, "ingest", formats[i % len(formats)]))
    for i, t in enumerate(query_times):
        events.append((t, "query", modes[i % len(modes)]))
    for t in delete_times:
        events.append((t, "delete", None))
    for t in reconcile_times:
        events.append((float(t), "reconcile", None))
    for t, kind in injection_schedule or []:
        events.append((float(t), "inject", kind))
    events.sort(key=lambda row: row[0])

    start = time.monotonic()
    idx = 0
    pending: list[concurrent.futures.Future[None]] = []

    with concurrent.futures.ThreadPoolExecutor(max_workers=max_workers) as pool:
        while True:
            elapsed = time.monotonic() - start
            if elapsed >= duration_seconds:
                break
            while idx < len(events) and events[idx][0] <= elapsed:
                _t, kind, arg = events[idx]
                idx += 1
                if kind == "inject":
                    if injection_callback is not None:
                        injection_callback(elapsed, str(arg), stats)
                    continue
                if kind == "ingest":
                    stats.mark_submitted("ingest")
                    pending.append(pool.submit(do_ingest, client, str(arg), stats, start_mono=start))
                elif kind == "query":
                    stats.mark_submitted("query")
                    pending.append(pool.submit(do_query, client, str(arg), stats, start_mono=start))
                elif kind == "delete":
                    stats.mark_submitted("delete")
                    pending.append(pool.submit(do_delete, client, stats, start_mono=start))
                elif kind == "reconcile":
                    doc = stats.document_ids[-1] if stats.document_ids else None
                    stats.mark_submitted("reconcile")
                    pending.append(
                        pool.submit(
                            do_reconcile,
                            compose_project=compose_project,
                            document_id=doc,
                            stats=stats,
                            start_mono=start,
                        )
                    )
            # Bound in-flight: collect completed and propagate exceptions.
            still: list[concurrent.futures.Future[None]] = []
            for fut in pending:
                if fut.done():
                    fut.result()
                else:
                    still.append(fut)
            pending = still
            if len(pending) >= max_workers:
                # Backpressure: wait for one completion before scheduling more.
                done, not_done = concurrent.futures.wait(
                    pending, return_when=concurrent.futures.FIRST_COMPLETED, timeout=0.2
                )
                for fut in done:
                    fut.result()
                pending = list(not_done)
            else:
                time.sleep(0.01)
        # Drain remaining futures — raise if any worker failed.
        for fut in pending:
            fut.result(timeout=client.timeout_seconds + 5)

    return stats


def metrics_from_stats(
    stats: RequestStats,
    duration_seconds: int,
    *,
    modes: list[str],
) -> dict[str, Any]:
    hours = max(duration_seconds, 1) / 3600.0
    success_latencies = list(stats.query_success_latencies_ms)
    mode_ok = {m: stats.query_success_by_mode.get(m, 0) for m in modes}
    mode_ready = all(mode_ok.get(m, 0) > 0 for m in modes) if modes else False
    query_p50 = percentile(success_latencies, 50) if success_latencies else None
    query_p95 = percentile(success_latencies, 95) if success_latencies else None
    query_p99 = percentile(success_latencies, 99) if success_latencies else None
    return {
        "scheduled": dict(stats.scheduled),
        "submitted": dict(stats.submitted),
        "completed": dict(stats.completed),
        "success": dict(stats.success),
        "requestErrors": stats.errors,
        "requestErrorsOutsideInjection": stats.errors_outside_injection,
        "requestErrorsInInjection": stats.errors_in_injection,
        "queryP50Ms": query_p50,
        "queryP95Ms": query_p95,
        "queryP99Ms": query_p99,
        "querySuccessSamples": len(success_latencies),
        "querySuccessByMode": mode_ok,
        "queryModesReady": mode_ready,
        "ingestDocsPerHour": stats.ingest_ok / hours if stats.ingest_ok else 0.0,
        "ingestOk": stats.ingest_ok,
        "deletedCount": len(stats.deleted_ids),
        "retainedCount": len(stats.retained_ids),
        "notReady": list(stats.not_ready),
        "durationSeconds": duration_seconds,
    }


def completeness_ok(
    stats: RequestStats,
    *,
    ratio: float = COMPLETENESS_RATIO,
) -> dict[str, Any]:
    """Require >= ratio of scheduled successes per actor (query/ingest), outside errors."""
    details: dict[str, Any] = {}
    ok = True
    for kind in ("ingest", "query"):
        scheduled = int(stats.scheduled.get(kind, 0))
        success = int(stats.success.get(kind, 0))
        need = int(scheduled * ratio) if scheduled else 0
        passed = success >= need if scheduled else False
        details[kind] = {
            "scheduled": scheduled,
            "success": success,
            "required": need,
            "passed": passed,
        }
        if scheduled and not passed:
            ok = False
    return {"passed": ok, "ratio": ratio, "actors": details}


def post_restore_retrieval_check(
    client: ApiClient,
    *,
    retained_ids: list[str],
    deleted_ids: list[str],
    same_run_restore: bool,
) -> dict[str, Any]:
    """After a same-run restore: retained authorized docs visible; deleted suppressed."""
    if not same_run_restore:
        return {
            "passed": None,
            "reason": "no_same_run_restore",
            "gate": "unknown",
        }
    if not retained_ids:
        return {"passed": False, "reason": "no_retained_ids", "gate": "fail"}
    if not deleted_ids:
        return {"passed": False, "reason": "no_deleted_ids", "gate": "fail"}
    body = json.dumps(
        {
            "query": "markhand soak post-restore",
            "mode": "current",
            "limit": 20,
            "collectionIds": [client.collection_id],
        }
    ).encode("utf-8")
    status, data, _latency = client.request("POST", "/api/v1/search", body=body)
    if not _http_success(status):
        return {"passed": False, "reason": f"search_status_{status}", "gate": "fail"}
    try:
        payload = json.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError):
        return {"passed": False, "reason": "invalid_json", "gate": "fail"}
    hits = payload.get("hits") or []
    hit_docs: set[str] = set()
    for hit in hits:
        if isinstance(hit, dict):
            for key in ("documentId", "document_id"):
                if hit.get(key):
                    hit_docs.add(str(hit.get(key)))
    leaked = [d for d in deleted_ids if d in hit_docs]
    retained_hit = any(r in hit_docs for r in retained_ids)
    # Retained may not appear in top hits for weak embeddings — also GET document.
    if not retained_hit:
        for rid in retained_ids:
            st, _b, _l = client.request("GET", f"/api/v1/documents/{rid}")
            if _http_success(st):
                retained_hit = True
                break
    passed = retained_hit and not leaked
    return {
        "passed": passed,
        "gate": "pass" if passed else "fail",
        "leakedDeletedIds": len(leaked),
        "retainedVisible": retained_hit,
        "hitCount": len(hits),
        "sameRunRestore": True,
    }
