"""Timed concurrent mixed-load against the configurable POC API."""

from __future__ import annotations

import concurrent.futures
import json
import math
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

import fixtures
from fixtures import fixture_path, marker_for, preflight_fixtures
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
    # actor -> reason -> count, so a failed run says why instead of only how many.
    failure_reasons: dict[str, dict[str, int]] = field(default_factory=dict)
    query_success_latencies_ms: list[float] = field(default_factory=list)
    query_success_by_mode: dict[str, int] = field(default_factory=dict)
    query_attempts_by_mode: dict[str, int] = field(default_factory=dict)
    ingest_ok: int = 0
    document_ids: list[str] = field(default_factory=list)
    deleted_ids: list[str] = field(default_factory=list)
    retained_ids: list[str] = field(default_factory=list)
    versions: dict[str, list[DocVersion]] = field(default_factory=dict)
    doc_markers: dict[str, str] = field(default_factory=dict)
    doc_versions: dict[str, str] = field(default_factory=dict)
    doc_effective_from: dict[str, str] = field(default_factory=dict)
    not_ready: list[str] = field(default_factory=list)
    exceptions: list[str] = field(default_factory=list)
    injection_windows: list[tuple[float, float]] = field(default_factory=list)
    compare_dataset: dict[str, str] | None = None
    # Optional external window checker (InjectionPlan.in_window).
    injection_window_fn: Callable[[float], bool] | None = None
    workload_start_mono: float | None = None
    workload_end_mono: float | None = None

    def mark_scheduled(self, kind: str, n: int = 1) -> None:
        with self.lock:
            self.scheduled[kind] = self.scheduled.get(kind, 0) + n

    def mark_submitted(self, kind: str) -> None:
        with self.lock:
            self.submitted[kind] = self.submitted.get(kind, 0) + 1

    def in_injection_window(self, monotonic_offset: float) -> bool:
        if self.injection_window_fn is not None:
            return bool(self.injection_window_fn(monotonic_offset))
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
        reason: str | None = None,
    ) -> None:
        with self.lock:
            self.completed[kind] = self.completed.get(kind, 0) + 1
            if not_ready_reason:
                self.not_ready.append(not_ready_reason)
            if ok:
                self.success[kind] = self.success.get(kind, 0) + 1
            else:
                label = reason or not_ready_reason or "unspecified"
                if in_injection:
                    label = f"{label}@injection"
                per_kind = self.failure_reasons.setdefault(kind, {})
                per_kind[label] = per_kind.get(label, 0) + 1
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

    def record_marker(self, document_id: str, marker: str) -> None:
        with self.lock:
            self.doc_markers[document_id] = marker

    def record_expected_version(self, document_id: str, version_id: str) -> None:
        with self.lock:
            self.doc_versions[document_id] = version_id

    def record_effective_from(self, document_id: str, effective_from: str) -> None:
        with self.lock:
            self.doc_effective_from[document_id] = effective_from

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

    def current_doc_marker(self) -> tuple[str, str] | None:
        with self.lock:
            for doc_id in self.retained_ids + self.document_ids:
                marker = self.doc_markers.get(doc_id)
                if marker:
                    return doc_id, marker
            if self.document_ids:
                return self.document_ids[0], ""
        return None


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


def _multipart_bytes(
    *,
    filename: str,
    file_bytes: bytes,
    collection_id: str,
    document_id: str | None = None,
) -> tuple[bytes, str]:
    boundary = f"----markhandsoak{uuid.uuid4().hex}"
    content_type = mimetypes.guess_type(filename)[0] or "application/octet-stream"
    chunks = [
        f"--{boundary}\r\n".encode(),
        (
            f'Content-Disposition: form-data; name="file"; filename="{filename}"\r\n'
            f"Content-Type: {content_type}\r\n\r\n"
        ).encode(),
        file_bytes,
        b"\r\n",
        f"--{boundary}\r\n".encode(),
        (
            f'Content-Disposition: form-data; name="collectionId"\r\n\r\n'
            f"{collection_id}\r\n"
        ).encode(),
    ]
    if document_id is not None:
        chunks.extend(
            [
                f"--{boundary}\r\n".encode(),
                (
                    f'Content-Disposition: form-data; name="documentId"\r\n\r\n'
                    f"{document_id}\r\n"
                ).encode(),
            ]
        )
    chunks.append(f"--{boundary}--\r\n".encode())
    return b"".join(chunks), f"multipart/form-data; boundary={boundary}"


def _multipart(
    file_path: Path,
    collection_id: str,
    document_id: str | None = None,
) -> tuple[bytes, str]:
    return _multipart_bytes(
        filename=file_path.name,
        file_bytes=file_path.read_bytes(),
        collection_id=collection_id,
        document_id=document_id,
    )


def _http_success(status: int) -> bool:
    return 200 <= status < 300


def _json_payload(data: bytes) -> dict[str, Any] | None:
    try:
        payload = json.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError):
        return None
    return payload if isinstance(payload, dict) else None


def _hit_doc_ids(payload: dict[str, Any]) -> set[str]:
    hits = payload.get("hits") or []
    docs: set[str] = set()
    for hit in hits:
        if isinstance(hit, dict):
            doc = hit.get("documentId") or hit.get("document_id") or hit.get("id")
            if doc:
                docs.add(str(doc))
    return docs


def _has_citation_for(
    payload: dict[str, Any],
    doc_id: str | None,
    *,
    version_id: str | None = None,
    marker: str | None = None,
) -> bool:
    citations = payload.get("citations") or []
    if not citations:
        return False
    if doc_id is None:
        return True
    for citation in citations:
        if not isinstance(citation, dict):
            continue
        ref = (
            citation.get("logicalDocumentId")
            or citation.get("documentId")
            or citation.get("document_id")
            or citation.get("docId")
        )
        cite_version = citation.get("versionId") or citation.get("version_id")
        quote = citation.get("quote")
        if ref is not None and str(ref) != doc_id:
            continue
        if version_id is not None and str(cite_version) != version_id:
            continue
        if marker is not None and (not isinstance(quote, str) or marker not in quote):
            continue
        if ref is not None:
            return True
    return False


def _payload_contains_marker(payload: dict[str, Any], marker: str | None) -> bool:
    if not marker:
        return True
    text = json.dumps(payload, ensure_ascii=False)
    return marker in text


def search_matches_expected(
    data: bytes,
    *,
    expected_doc: str | None,
    expected_version: str | None = None,
    expected_marker: str | None,
    require_citation: bool = True,
) -> bool:
    payload = _json_payload(data)
    if payload is None:
        return False
    if expected_doc:
        matching_hits = []
        for hit in payload.get("hits") or []:
            if not isinstance(hit, dict):
                continue
            doc = hit.get("documentId") or hit.get("document_id") or hit.get("id")
            version = hit.get("versionId") or hit.get("version_id")
            snippet = hit.get("snippet") or hit.get("quote") or hit.get("body")
            if str(doc) != expected_doc:
                continue
            if expected_version is not None and str(version) != expected_version:
                continue
            if expected_marker is not None and (
                not isinstance(snippet, str) or expected_marker not in snippet
            ):
                continue
            matching_hits.append(hit)
        if not matching_hits:
            return False
    else:
        if not _hit_doc_ids(payload):
            return False
    if require_citation and not _has_citation_for(
        payload, expected_doc, version_id=expected_version, marker=expected_marker
    ):
        return False
    return True


def wait_until_indexed_visible(
    client: ApiClient,
    *,
    document_id: str,
    marker: str,
    timeout_seconds: float,
    poll_seconds: float = 2.0,
    expected_version: str | None = None,
) -> bool:
    deadline = time.monotonic() + timeout_seconds
    while time.monotonic() < deadline:
        body = json.dumps(
            {
                "query": marker,
                "mode": "current",
                "limit": 5,
                "collectionIds": [client.collection_id],
            }
        ).encode("utf-8")
        status, data, _latency = client.request("POST", "/api/v1/search", body=body)
        if _http_success(status) and search_matches_expected(
            data,
            expected_doc=document_id,
            expected_version=expected_version,
            expected_marker=marker,
            require_citation=True,
        ):
            return True
        time.sleep(poll_seconds)
    return False


def current_published_version(client: ApiClient, document_id: str) -> str | None:
    status, data, _latency = client.request(
        "GET", f"/api/v1/documents/{document_id}/versions"
    )
    if not _http_success(status):
        return None
    payload = _json_payload(data)
    items = payload.get("items") if payload is not None else None
    if not isinstance(items, list):
        return None
    current = [
        str(row["id"])
        for row in items
        if isinstance(row, dict)
        and row.get("isCurrent") is True
        and isinstance(row.get("id"), str)
    ]
    return current[0] if len(current) == 1 else None


def do_ingest(
    client: ApiClient,
    fmt: str,
    stats: RequestStats,
    *,
    start_mono: float,
    scheduled_mono: float | None = None,
) -> None:
    # Every upload carries its own marker so the retrieval assertion targets one
    # exact document instead of racing the whole collection for a top-N slot.
    # PNG is the exception when Pillow is unavailable: its marker has to survive
    # OCR, and the bitmap fallback is unreadable, so it keeps the golden marker.
    unique_marker_used = fmt.lower() != "png" or fixtures.unique_png_marker_supported()
    if unique_marker_used:
        marker = fixtures.unique_marker(fmt, uuid.uuid4().hex[:8].upper())
        file_bytes = fixtures.generate_bytes(fmt, marker)
    else:
        marker = marker_for(fmt)
        file_bytes = fixture_path(fmt).read_bytes()
    body, content_type = _multipart_bytes(
        filename=fixtures.fixture_filename(fmt),
        file_bytes=file_bytes,
        collection_id=client.collection_id,
    )
    status, data, _latency = client.request(
        "POST",
        "/api/v1/uploads",
        body=body,
        headers=client._headers(content_type),
    )
    doc_id = None
    version_id = None
    ok = _http_success(status)
    reason: str | None = None if ok else f"upload_http_{status}"
    if ok:
        payload = _json_payload(data)
        if payload is None:
            ok = False
            reason = "upload_body_not_json"
        else:
            doc_id = payload.get("documentId")
            version_id = payload.get("versionId")
    if ok and isinstance(doc_id, str) and isinstance(version_id, str):
        # Each upload is a new documentId — never invent a second version pair.
        ok = wait_until_indexed_visible(
            client,
            document_id=doc_id,
            marker=marker,
            timeout_seconds=float(os.environ.get("MARKHAND_SOAK_INGEST_TERMINAL_TIMEOUT", "180")),
        )
        if not ok:
            reason = "not_visible_before_timeout"
        published_version = current_published_version(client, doc_id) if ok else None
        if ok and published_version is None:
            reason = "no_published_version"
        ok = published_version is not None
        if published_version is not None:
            stats.record_version(doc_id, published_version, published=True)
            stats.record_marker(doc_id, marker)
            stats.record_expected_version(doc_id, published_version)
    elif ok:
        ok = False
        reason = "upload_missing_document_or_version_id"
    in_inj = stats.in_injection_window(time.monotonic() - start_mono)
    stats.add(
        "ingest",
        ok=ok,
        doc_id=doc_id if isinstance(doc_id, str) else None,
        in_injection=in_inj,
        reason=reason,
    )


def do_query(
    client: ApiClient,
    mode: str,
    stats: RequestStats,
    *,
    start_mono: float,
    scheduled_mono: float | None = None,
) -> None:
    body_obj: dict[str, Any] = {
        "query": "markhand soak synthetic query",
        "mode": mode,
        "limit": 5,
        "collectionIds": [client.collection_id],
    }
    not_ready = None
    expected_doc = None
    expected_version = None
    expected_marker = None
    if mode == "as_of":
        dataset = stats.compare_dataset
        if not dataset:
            not_ready = "as_of_dataset_unavailable"
        else:
            doc = dataset["documentId"]
            body_obj["asOf"] = dataset["asOfA"]
            body_obj["documentId"] = doc
            expected_doc = doc
            expected_version = dataset["versionA"]
            expected_marker = dataset["markerA"]
            body_obj["query"] = expected_marker
            body_obj["expectedSeededEffectiveFrom"] = dataset["effectiveFromA"]
    elif mode == "compare":
        dataset = stats.compare_dataset
        if not dataset:
            not_ready = "compare_dataset_unavailable"
        else:
            body_obj["documentId"] = dataset["documentId"]
            body_obj["versionA"] = dataset["versionA"]
            body_obj["versionB"] = dataset["versionB"]
            expected_doc = dataset["documentId"]
            body_obj["query"] = dataset["query"]
            # Compare is verified separately against both versions; this request
            # must at least bind to the supplied logical document.
    elif mode == "current":
        current = stats.current_doc_marker()
        if not current:
            not_ready = "current_no_indexed_document"
        else:
            expected_doc, expected_marker = current
            expected_version = stats.doc_versions.get(expected_doc)
            if expected_marker:
                body_obj["query"] = expected_marker
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
    status, data, _latency = client.request("POST", "/api/v1/search", body=body)
    http_ok = _http_success(status)
    ok = http_ok and search_matches_expected(
        data,
        expected_doc=expected_doc,
        expected_version=locals().get("expected_version"),
        expected_marker=expected_marker,
        require_citation=True,
    )
    reason = None
    if not ok:
        reason = f"search_http_{status}" if not http_ok else f"no_expected_match:{mode}"
    stats.add(
        "query",
        ok=ok,
        latency_ms=((time.monotonic() - (scheduled_mono or start_mono)) * 1000.0) if ok else None,
        mode=mode,
        in_injection=in_inj,
        reason=reason,
    )


def do_delete(
    client: ApiClient,
    stats: RequestStats,
    *,
    start_mono: float,
    scheduled_mono: float | None = None,
) -> None:
    with stats.lock:
        if not stats.document_ids:
            doc_id = None
        elif stats.retained_ids:
            retained = set(stats.retained_ids)
            candidate = next(
                (
                    index
                    for index, document_id in enumerate(stats.document_ids)
                    if document_id not in retained
                ),
                None,
            )
            doc_id = (
                None if candidate is None else stats.document_ids.pop(candidate)
            )
        else:
            # Keep at least one retained doc for post-restore authorized retrieval.
            if len(stats.document_ids) <= 1:
                stats.retained_ids.append(stats.document_ids[0])
                doc_id = None
            else:
                doc_id = stats.document_ids.pop(0)
                if stats.document_ids:
                    stats.retained_ids.append(stats.document_ids[0])
    in_inj = stats.in_injection_window(time.monotonic() - start_mono)
    if not doc_id:
        stats.add("delete", ok=False, in_injection=in_inj, not_ready_reason="delete_no_doc")
        return
    status, _data, _latency = client.request("DELETE", f"/api/v1/documents/{doc_id}")
    delete_ok = _http_success(status)
    stats.add(
        "delete",
        ok=delete_ok,
        doc_id=doc_id,
        in_injection=in_inj,
        reason=None if delete_ok else f"delete_http_{status}",
    )


def do_reconcile(
    *,
    compose_project: str,
    document_id: str | None,
    stats: RequestStats,
    start_mono: float,
    runner: Callable[..., Any] | None = None,
    scheduled_mono: float | None = None,
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
    reason: str | None = None
    try:
        proc = run(cmd, capture_output=True, text=True, check=False, env=env, timeout=90)
        ok = proc.returncode == 0
        if not ok:
            reason = f"reconcile_exit_{proc.returncode}"
    except (OSError, subprocess.SubprocessError) as exc:
        ok = False
        reason = f"reconcile_spawn_{type(exc).__name__}"
    stats.add("reconcile", ok=ok, in_injection=in_inj, reason=reason)


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
    injection_callback: Callable[[float, str], None] | None = None,
    injection_schedule: list[tuple[float, str]] | None = None,
    compare_dataset: dict[str, str] | None = None,
    injection_window_fn: Callable[[float], bool] | None = None,
    retained_ids: list[str] | None = None,
    retained_markers: dict[str, str] | None = None,
    max_workers: int = 16,
    skip_fixture_preflight: bool = False,
) -> RequestStats:
    """Execute scheduled ingest/query/delete/reconcile for ``duration_seconds``.

    Sampler and injection must run out-of-band (dedicated threads). This loop only
    dispatches work on monotonic time. Injection callback must return immediately
    (schedule onto an executor). Futures are drained via ``result()``.
    """
    formats = list(profile["actors"]["ingest"]["formats"])
    if not skip_fixture_preflight:
        preflight_fixtures(formats)

    stats = RequestStats()
    stats.compare_dataset = compare_dataset
    stats.injection_window_fn = injection_window_fn
    if retained_ids:
        stats.retained_ids = list(retained_ids)
        stats.document_ids = list(retained_ids)
    if retained_markers:
        stats.doc_markers.update({str(k): str(v) for k, v in retained_markers.items()})
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
    stats.workload_start_mono = start
    idx = 0
    pending: list[concurrent.futures.Future[None]] = []
    per_actor_workers = max(1, max_workers // 4)

    with (
        concurrent.futures.ThreadPoolExecutor(max_workers=per_actor_workers, thread_name_prefix="o05-ingest") as ingest_pool,
        concurrent.futures.ThreadPoolExecutor(max_workers=per_actor_workers, thread_name_prefix="o05-query") as query_pool,
        concurrent.futures.ThreadPoolExecutor(max_workers=per_actor_workers, thread_name_prefix="o05-delete") as delete_pool,
        concurrent.futures.ThreadPoolExecutor(max_workers=1, thread_name_prefix="o05-reconcile") as reconcile_pool,
    ):
        while True:
            elapsed = time.monotonic() - start
            if elapsed >= duration_seconds:
                break
            while idx < len(events) and events[idx][0] <= elapsed:
                _t, kind, arg = events[idx]
                idx += 1
                if kind == "inject":
                    if injection_callback is not None:
                        # Must be non-blocking: schedule onto injection executor.
                        injection_callback(elapsed, str(arg))
                    continue
                if kind == "ingest":
                    stats.mark_submitted("ingest")
                    pending.append(
                        ingest_pool.submit(
                            do_ingest,
                            client,
                            str(arg),
                            stats,
                            start_mono=start,
                            scheduled_mono=start + float(_t),
                        )
                    )
                elif kind == "query":
                    stats.mark_submitted("query")
                    pending.append(
                        query_pool.submit(
                            do_query,
                            client,
                            str(arg),
                            stats,
                            start_mono=start,
                            scheduled_mono=start + float(_t),
                        )
                    )
                elif kind == "delete":
                    stats.mark_submitted("delete")
                    pending.append(
                        delete_pool.submit(
                            do_delete,
                            client,
                            stats,
                            start_mono=start,
                            scheduled_mono=start + float(_t),
                        )
                    )
                elif kind == "reconcile":
                    doc = stats.document_ids[-1] if stats.document_ids else None
                    stats.mark_submitted("reconcile")
                    pending.append(
                        reconcile_pool.submit(
                            do_reconcile,
                            compose_project=compose_project,
                            document_id=doc,
                            stats=stats,
                            start_mono=start,
                            scheduled_mono=start + float(_t),
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

    stats.workload_end_mono = time.monotonic()
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
        "failureReasons": {
            kind: dict(sorted(reasons.items(), key=lambda item: -item[1]))
            for kind, reasons in sorted(stats.failure_reasons.items())
        },
        "durationSeconds": duration_seconds,
        "actualElapsedSeconds": (
            None
            if stats.workload_start_mono is None or stats.workload_end_mono is None
            else round(stats.workload_end_mono - stats.workload_start_mono, 3)
        ),
    }


def completeness_ok(
    stats: RequestStats,
    *,
    ratio: float = COMPLETENESS_RATIO,
) -> dict[str, Any]:
    """Require scheduled work to drain and actor success to meet qualification minima."""
    details: dict[str, Any] = {}
    ok = True
    for kind in ("ingest", "query", "delete", "reconcile"):
        scheduled = int(stats.scheduled.get(kind, 0))
        submitted = int(stats.submitted.get(kind, 0))
        completed = int(stats.completed.get(kind, 0))
        success = int(stats.success.get(kind, 0))
        if kind == "reconcile":
            need = scheduled
        else:
            need = math.ceil(scheduled * ratio) if scheduled else 0
        drained = scheduled == submitted == completed
        passed = bool(scheduled and drained and success >= need)
        details[kind] = {
            "scheduled": scheduled,
            "submitted": submitted,
            "completed": completed,
            "success": success,
            "required": need,
            "drained": drained,
            "passed": passed,
        }
        if not passed:
            ok = False
    return {
        "passed": ok,
        "drainPassed": all(v["drained"] and v["scheduled"] > 0 for v in details.values()),
        "reconcilePassed": details["reconcile"]["passed"],
        "ratio": ratio,
        "actors": details,
    }


def post_restore_retrieval_check(*args: Any, **kwargs: Any) -> dict[str, Any]:
    """Compatibility wrapper — real implementation lives in ``dataset``."""
    from dataset import post_restore_retrieval_check as _impl

    return _impl(*args, **kwargs)
