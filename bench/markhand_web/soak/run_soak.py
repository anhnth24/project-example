#!/usr/bin/env python3
"""Phase-1B mixed-load soak harness.

The harness is live-capable but intentionally self-skips when no target URL and
bearer token are configured. Numeric G0 gates require a real sustained stack
with targetMatch=true; sandbox/self-skip output is only harness evidence.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import datetime as dt
import hashlib
import json
import os
import platform
import random
import re
import statistics
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[3]
CORPUS = ROOT / "bench/markhand_web"
DEFAULT_PROFILE = CORPUS / "workloads/soak-smoke.yaml"
SUMMARY_PATH = CORPUS / "soak/summary.json"
REPORT_PATH = CORPUS / "reports/phase-1b-gate/soak.md"
GATES_PATH = CORPUS / "gates.yaml"
WORKLOAD_PROFILE = CORPUS / "workload-profile.yaml"
DOES_NOT_CLAIM = (
    "does NOT claim numeric G0-SLO/G0-CAP/soak pass evidence without sustained real infra"
)
TERMINAL_JOB_STATUSES = {"succeeded", "failed", "cancelled", "dead_letter"}
SUCCESS_JOB_STATUSES = {"succeeded"}
MONITORED_MARKHAND_METRICS = [
    "markhand_http_requests_total",
    "markhand_http_request_duration_seconds_bucket",
    "markhand_http_request_duration_seconds_sum",
    "markhand_http_request_duration_seconds_count",
    "markhand_jobs_processed_total",
    "markhand_job_duration_seconds_bucket",
    "markhand_job_duration_seconds_sum",
    "markhand_job_duration_seconds_count",
    "markhand_jobs_in_flight",
    "markhand_jobs_queue_depth",
    "markhand_retrieval_latency_seconds_bucket",
    "markhand_retrieval_latency_seconds_sum",
    "markhand_retrieval_latency_seconds_count",
    "markhand_embedding_latency_seconds_bucket",
    "markhand_embedding_latency_seconds_sum",
    "markhand_embedding_latency_seconds_count",
]
OPTIONAL_LEAK_PROXY_METRICS = [
    "markhand_process_resident_memory_bytes",
    "process_resident_memory_bytes",
    "markhand_temp_bytes",
    "markhand_open_connections",
    "markhand_jobs_dead_letter_total",
    "markhand_jobs_dead_letter_depth",
]
IMPLEMENTATION_FILES = (
    "bench/markhand_web/soak/run_soak.py",
    "bench/markhand_web/workloads/soak-smoke.yaml",
    "bench/markhand_web/workloads/soak-phase-1b.yaml",
)


class HarnessError(RuntimeError):
    """Actionable soak harness error."""


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z")


def relative(path: Path) -> str:
    try:
        return str(path.resolve().relative_to(ROOT))
    except ValueError:
        return str(path)


def load_json(path: Path) -> dict[str, Any]:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise HarnessError(f"{relative(path)}: cannot load JSON-compatible YAML: {error}") from error
    if not isinstance(payload, dict):
        raise HarnessError(f"{relative(path)}: expected object")
    return payload


def file_sha256(path: Path) -> str | None:
    if not path.is_file():
        return None
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def implementation_sha256() -> str:
    digest = hashlib.sha256()
    for rel in IMPLEMENTATION_FILES:
        path = ROOT / rel
        digest.update(rel.encode())
        digest.update(b"\0")
        digest.update(path.read_bytes() if path.is_file() else b"missing")
        digest.update(b"\0")
    return digest.hexdigest()


def git(*args: str) -> str:
    try:
        return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def git_status() -> dict[str, Any]:
    raw = ""
    try:
        raw = subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT, text=True)
    except (OSError, subprocess.CalledProcessError):
        pass
    dirty_paths: list[str] = []
    for line in raw.splitlines():
        if len(line) < 4:
            continue
        path = line[3:]
        if " -> " in path:
            path = path.split(" -> ", 1)[1]
        if path.startswith('"') and path.endswith('"'):
            path = path[1:-1]
        dirty_paths.append(path)
    return {
        "commit": git("rev-parse", "HEAD"),
        "branch": git("branch", "--show-current"),
        "dirty": bool(dirty_paths),
        "dirtyPaths": dirty_paths,
    }


def command_version(args: list[str], timeout: float = 5.0) -> dict[str, Any]:
    try:
        completed = subprocess.run(
            args,
            cwd=ROOT,
            text=True,
            capture_output=True,
            timeout=timeout,
        )
    except (OSError, subprocess.SubprocessError) as error:
        return {"available": False, "argv": args, "error": str(error)}
    output = (completed.stdout or completed.stderr).strip()
    return {
        "available": completed.returncode == 0,
        "argv": args,
        "exitCode": completed.returncode,
        "output": output[:500],
    }


def stack_versions() -> dict[str, Any]:
    return {
        "python": sys.version.split()[0],
        "platform": platform.platform(),
        "docker": command_version(["docker", "--version"]),
        "dockerCompose": command_version(["docker", "compose", "version"]),
        "profileSha256": file_sha256(DEFAULT_PROFILE),
        "gatesSha256": file_sha256(GATES_PATH),
        "workloadProfileSha256": file_sha256(WORKLOAD_PROFILE),
    }


def percentile(values: list[float], pct: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    if len(ordered) == 1:
        return round(ordered[0], 3)
    rank = (len(ordered) - 1) * pct
    lower = int(rank)
    upper = min(lower + 1, len(ordered) - 1)
    fraction = rank - lower
    return round(ordered[lower] + (ordered[upper] - ordered[lower]) * fraction, 3)


def duration_stats(values: list[float]) -> dict[str, Any]:
    if not values:
        return {"count": 0, "min": None, "mean": None, "p50": None, "p95": None, "p99": None, "max": None}
    return {
        "count": len(values),
        "min": round(min(values), 3),
        "mean": round(statistics.fmean(values), 3),
        "p50": percentile(values, 0.50),
        "p95": percentile(values, 0.95),
        "p99": percentile(values, 0.99),
        "max": round(max(values), 3),
    }


def weighted_choice(weights: dict[str, Any], rng: random.Random) -> str:
    items = [(str(name), float(weight)) for name, weight in weights.items() if float(weight) > 0]
    if not items:
        raise HarnessError("profile operationWeights must contain at least one positive weight")
    total = sum(weight for _, weight in items)
    needle = rng.random() * total
    upto = 0.0
    for name, weight in items:
        upto += weight
        if needle <= upto:
            return name
    return items[-1][0]


def parse_labels(raw: str) -> dict[str, str]:
    labels: dict[str, str] = {}
    for match in re.finditer(r'([a-zA-Z_][a-zA-Z0-9_]*)="((?:\\.|[^"])*)"', raw):
        labels[match.group(1)] = match.group(2).replace(r"\"", '"').replace(r"\\", "\\")
    return labels


def parse_prometheus(text: str) -> dict[str, Any]:
    samples: list[dict[str, Any]] = []
    families: set[str] = set()
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        match = re.match(
            r"^([a-zA-Z_:][a-zA-Z0-9_:]*)(?:\{([^}]*)\})?\s+"
            r"([-+]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][-+]?\d+)?|NaN|Inf|-Inf)(?:\s+\d+)?$",
            line,
        )
        if not match:
            continue
        name, raw_labels, raw_value = match.groups()
        if not (name.startswith("markhand_") or name in OPTIONAL_LEAK_PROXY_METRICS):
            continue
        try:
            value = float(raw_value)
        except ValueError:
            continue
        families.add(name)
        samples.append({"name": name, "labels": parse_labels(raw_labels or ""), "value": value})
    gauges: dict[str, float] = {}
    for sample in samples:
        if sample["labels"]:
            continue
        gauges[sample["name"]] = sample["value"]
    return {"families": sorted(families), "samples": samples, "gauges": gauges}


def multipart_body(filename: str, content_type: str, content: bytes) -> tuple[bytes, str]:
    boundary = f"----markhand-soak-{uuid.uuid4().hex}"
    body = bytearray()
    body.extend(
        (
            f"--{boundary}\r\n"
            f'Content-Disposition: form-data; name="file"; filename="{filename}"\r\n'
            f"Content-Type: {content_type}\r\n\r\n"
        ).encode()
    )
    body.extend(content)
    body.extend(f"\r\n--{boundary}--\r\n".encode())
    return bytes(body), f"multipart/form-data; boundary={boundary}"


class ApiClient:
    def __init__(self, base_url: str, token: str, timeout_seconds: float) -> None:
        self.base_url = base_url.rstrip("/")
        self.token = token
        self.timeout_seconds = timeout_seconds

    def request(
        self,
        method: str,
        path: str,
        *,
        json_body: Any | None = None,
        body: bytes | None = None,
        content_type: str | None = None,
        extra_headers: dict[str, str] | None = None,
    ) -> dict[str, Any]:
        url = urllib.parse.urljoin(f"{self.base_url}/", path.lstrip("/"))
        data = body
        headers = {
            "Authorization": f"Bearer {self.token}",
            "User-Agent": "markhand-phase-1b-soak/1",
        }
        if json_body is not None:
            data = json.dumps(json_body, separators=(",", ":")).encode()
            headers["Content-Type"] = "application/json"
        elif content_type:
            headers["Content-Type"] = content_type
        if extra_headers:
            headers.update(extra_headers)
        started = time.perf_counter()
        status: int | None = None
        response_text = ""
        error = None
        try:
            request = urllib.request.Request(url, data=data, method=method.upper(), headers=headers)
            with urllib.request.urlopen(request, timeout=self.timeout_seconds) as response:
                status = int(response.status)
                response_text = response.read(2 * 1024 * 1024).decode("utf-8", errors="replace")
        except urllib.error.HTTPError as exc:
            status = int(exc.code)
            response_text = exc.read(2 * 1024 * 1024).decode("utf-8", errors="replace")
        except (urllib.error.URLError, TimeoutError, OSError) as exc:
            error = str(exc)
        elapsed_ms = round((time.perf_counter() - started) * 1000, 3)
        parsed: Any = None
        if response_text:
            try:
                parsed = json.loads(response_text)
            except json.JSONDecodeError:
                parsed = response_text[:2000]
        return {
            "method": method.upper(),
            "path": path,
            "status": status,
            "ok": status is not None and 200 <= status < 300,
            "durationMs": elapsed_ms,
            "json": parsed,
            "error": error,
        }

    def get(self, path: str) -> dict[str, Any]:
        return self.request("GET", path)

    def post_json(self, path: str, body: Any) -> dict[str, Any]:
        return self.request("POST", path, json_body=body)

    def delete(self, path: str, headers: dict[str, str] | None = None) -> dict[str, Any]:
        return self.request("DELETE", path, extra_headers=headers)

    def upload(self, filename: str, content_type: str, content: bytes) -> dict[str, Any]:
        data, multipart_content_type = multipart_body(filename, content_type, content)
        return self.request("POST", "/api/v1/uploads", body=data, content_type=multipart_content_type)


class SharedState:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.active_docs: list[dict[str, Any]] = []
        self.deleted_docs: list[dict[str, Any]] = []
        self.jobs: list[dict[str, Any]] = []

    def add_doc(self, doc: dict[str, Any]) -> None:
        with self.lock:
            self.active_docs.append(doc)

    def pick_doc(self) -> dict[str, Any] | None:
        with self.lock:
            if not self.active_docs:
                return None
            return random.choice(self.active_docs)

    def pop_doc(self) -> dict[str, Any] | None:
        with self.lock:
            if not self.active_docs:
                return None
            index = random.randrange(len(self.active_docs))
            return self.active_docs.pop(index)

    def mark_deleted(self, doc: dict[str, Any]) -> None:
        with self.lock:
            self.deleted_docs.append(doc)

    def add_job(self, job: dict[str, Any]) -> None:
        with self.lock:
            self.jobs.append(job)

    def snapshot(self) -> dict[str, int]:
        with self.lock:
            return {
                "activeDocuments": len(self.active_docs),
                "deletedDocuments": len(self.deleted_docs),
                "observedJobs": len(self.jobs),
            }


def ensure_collection(client: ApiClient, configured_collection_id: str | None) -> str:
    if configured_collection_id:
        return configured_collection_id
    suffix = uuid.uuid4().hex[:12]
    body = {
        "name": f"Phase 1B soak {suffix}",
        "slug": f"phase-1b-soak-{suffix}",
        "description": "Synthetic/redacted Phase-1B mixed-load soak collection.",
        "visibility": "private",
    }
    response = client.post_json("/api/v1/collections", body)
    if response["status"] != 201 or not isinstance(response["json"], dict):
        raise HarnessError(f"collection create failed status={response['status']} error={response['error']}")
    collection_id = response["json"].get("id")
    if not isinstance(collection_id, str):
        raise HarnessError("collection create response missing id")
    return collection_id


def poll_job(client: ApiClient, job_id: str, timeout_seconds: float, interval_seconds: float) -> dict[str, Any]:
    deadline = time.monotonic() + timeout_seconds
    attempts = 0
    last: dict[str, Any] | None = None
    while time.monotonic() < deadline:
        attempts += 1
        response = client.get(f"/api/v1/jobs/{job_id}")
        last = response
        if response["ok"] and isinstance(response["json"], dict):
            status = str(response["json"].get("status"))
            if status in TERMINAL_JOB_STATUSES:
                return {
                    "jobId": job_id,
                    "terminal": True,
                    "status": status,
                    "success": status in SUCCESS_JOB_STATUSES,
                    "attempts": attempts,
                    "lastStatus": response["status"],
                }
        if response["status"] in {401, 403, 404}:
            break
        time.sleep(interval_seconds)
    status = None
    if last and isinstance(last.get("json"), dict):
        status = last["json"].get("status")
    return {
        "jobId": job_id,
        "terminal": False,
        "status": status,
        "success": False,
        "attempts": attempts,
        "lastStatus": last.get("status") if last else None,
    }


def op_result(operation: str, status: str, started: float, **extra: Any) -> dict[str, Any]:
    payload = {
        "operation": operation,
        "status": status,
        "durationMs": round((time.perf_counter() - started) * 1000, 3),
        "at": utc_now(),
    }
    payload.update(extra)
    return payload


def ingest_operation(
    client: ApiClient,
    profile: dict[str, Any],
    collection_id: str,
    state: SharedState,
) -> dict[str, Any]:
    started = time.perf_counter()
    data = profile.get("data", {})
    prefix = str(data.get("fixturePrefix", "P1B-O05-SOAK"))
    marker = f"{prefix}-{uuid.uuid4().hex}"
    content = (
        f"{marker}\n"
        "Synthetic redacted Markhand Phase-1B soak document.\n"
        "This fixture contains no customer data or secrets.\n"
    ).encode("utf-8")
    filename = f"{marker}.txt"
    content_type = str(data.get("uploadContentType", "text/plain"))
    upload = client.upload(filename, content_type, content)
    if upload["status"] != 201 or not isinstance(upload["json"], dict):
        return op_result("ingest", "error", started, stage="upload", response=trim_response(upload))
    object_key = upload["json"].get("objectKey")
    if not isinstance(object_key, str):
        return op_result("ingest", "error", started, stage="upload", response="missing objectKey")
    create = client.post_json(
        f"/api/v1/collections/{collection_id}/documents",
        {"objectKey": object_key, "title": f"Synthetic soak {marker}"},
    )
    if create["status"] != 201 or not isinstance(create["json"], dict):
        return op_result("ingest", "error", started, stage="create", response=trim_response(create))
    document = create["json"].get("document", {})
    version = create["json"].get("version", {})
    job_id = create["json"].get("jobId")
    job = None
    if isinstance(job_id, str):
        job = poll_job(
            client,
            job_id,
            float(profile.get("jobPollTimeoutSeconds", 60)),
            float(profile.get("jobPollIntervalSeconds", 2)),
        )
        state.add_job(job)
    doc = {
        "id": document.get("id"),
        "versionId": version.get("id"),
        "jobId": job_id,
        "marker": marker,
        "title": document.get("title"),
    }
    if isinstance(doc["id"], str):
        state.add_doc(doc)
    success = job is None or bool(job.get("success"))
    return op_result(
        "ingest",
        "ok" if success else "partial",
        started,
        documentId=doc["id"],
        versionId=doc["versionId"],
        job=job,
    )


def query_text(profile: dict[str, Any], state: SharedState) -> str:
    doc = state.pick_doc()
    if doc and isinstance(doc.get("marker"), str) and random.random() < 0.5:
        return doc["marker"]
    queries = profile.get("data", {}).get("queries", [])
    if isinstance(queries, list) and queries:
        return str(random.choice(queries))
    return "Markhand Phase 1B soak"


def query_operation(
    client: ApiClient,
    profile: dict[str, Any],
    collection_id: str,
    state: SharedState,
) -> dict[str, Any]:
    started = time.perf_counter()
    body = {"query": query_text(profile, state), "collectionIds": [collection_id]}
    response = client.post_json("/api/v1/search", body)
    return op_result(
        "query_search",
        "ok" if response["ok"] else "error",
        started,
        httpStatus=response["status"],
        response=trim_response(response, include_body=False),
    )


def ask_operation(
    client: ApiClient,
    profile: dict[str, Any],
    collection_id: str,
    state: SharedState,
) -> dict[str, Any]:
    started = time.perf_counter()
    body = {"question": query_text(profile, state), "collectionIds": [collection_id]}
    response = client.post_json("/api/v1/ask", body)
    return op_result(
        "ask",
        "ok" if response["ok"] else "error",
        started,
        httpStatus=response["status"],
        response=trim_response(response, include_body=False),
    )


def delete_operation(client: ApiClient, state: SharedState) -> dict[str, Any]:
    started = time.perf_counter()
    doc = state.pop_doc()
    if not doc or not isinstance(doc.get("id"), str):
        return op_result("delete", "skipped", started, reason="no_active_document")
    response = client.delete(
        f"/api/v1/documents/{doc['id']}",
        headers={"Idempotency-Key": f"soak-delete-{doc['id']}-{uuid.uuid4().hex}"},
    )
    if response["ok"]:
        state.mark_deleted(doc)
    else:
        state.add_doc(doc)
    return op_result(
        "delete",
        "ok" if response["ok"] else "error",
        started,
        documentId=doc["id"],
        httpStatus=response["status"],
        response=trim_response(response, include_body=False),
    )


def reconcile_operation(client: ApiClient, state: SharedState) -> dict[str, Any]:
    """Exercise the public consistency leg available to HTTP callers.

    There is no public reconcile REST route in P1B. The actual reconcile worker is
    observed through metrics and O03 restore-fence steps; this live HTTP leg
    enqueues reindex when a document exists and otherwise probes readiness.
    """

    started = time.perf_counter()
    doc = state.pick_doc()
    if doc and isinstance(doc.get("id"), str):
        response = client.post_json(f"/api/v1/documents/{doc['id']}:reindex", {})
        if response["ok"] and isinstance(response["json"], dict):
            job_id = response["json"].get("id")
            if isinstance(job_id, str):
                state.add_job({"jobId": job_id, "status": "queued_by_reconcile_leg", "success": None})
        return op_result(
            "reconcile",
            "ok" if response["ok"] else "error",
            started,
            leg="document_reindex_public_proxy",
            documentId=doc["id"],
            httpStatus=response["status"],
            response=trim_response(response, include_body=False),
        )
    response = client.get("/api/v1/health/ready")
    return op_result(
        "reconcile",
        "ok" if response["ok"] else "error",
        started,
        leg="readiness_fence_probe",
        httpStatus=response["status"],
        response=trim_response(response, include_body=False),
    )


def trim_response(response: dict[str, Any], include_body: bool = True) -> dict[str, Any]:
    trimmed = {
        "method": response.get("method"),
        "path": response.get("path"),
        "status": response.get("status"),
        "durationMs": response.get("durationMs"),
        "error": response.get("error"),
    }
    if include_body:
        body = response.get("json")
        if isinstance(body, dict):
            redacted = {key: value for key, value in body.items() if key not in {"accessToken", "refreshToken"}}
            trimmed["body"] = redacted
        elif body is not None:
            trimmed["body"] = str(body)[:500]
    return trimmed


def sampler(client: ApiClient, interval: float, stop: threading.Event, output: list[dict[str, Any]]) -> None:
    while not stop.is_set():
        sample_started = time.perf_counter()
        ready = client.get("/api/v1/health/ready")
        metrics = client.get("/api/v1/metrics")
        parsed = parse_prometheus(metrics.get("json") if isinstance(metrics.get("json"), str) else "")
        output.append(
            {
                "at": utc_now(),
                "ready": {
                    "status": ready["status"],
                    "ok": ready["ok"],
                    "durationMs": ready["durationMs"],
                },
                "metrics": {
                    "status": metrics["status"],
                    "ok": metrics["ok"],
                    "durationMs": metrics["durationMs"],
                    "families": parsed["families"],
                    "gauges": parsed["gauges"],
                    "samples": parsed["samples"],
                },
            }
        )
        elapsed = time.perf_counter() - sample_started
        stop.wait(max(0.1, interval - elapsed))


def run_worker(
    worker_index: int,
    client: ApiClient,
    profile: dict[str, Any],
    collection_id: str,
    state: SharedState,
    deadline: float,
    ramp_seconds: float,
    results: list[dict[str, Any]],
    result_lock: threading.Lock,
) -> None:
    rng = random.Random(20260720 + worker_index)
    concurrency = max(1, int(profile.get("concurrency", 1)))
    if ramp_seconds > 0:
        time.sleep((worker_index / concurrency) * ramp_seconds)
    weights = profile.get("operationWeights", {})
    think_time = float(profile.get("thinkTimeSeconds", 0.05))
    while time.monotonic() < deadline:
        operation = weighted_choice(weights, rng)
        try:
            if operation == "ingest":
                result = ingest_operation(client, profile, collection_id, state)
            elif operation == "query_search":
                result = query_operation(client, profile, collection_id, state)
            elif operation == "ask":
                result = ask_operation(client, profile, collection_id, state)
            elif operation == "delete":
                result = delete_operation(client, state)
            elif operation == "reconcile":
                result = reconcile_operation(client, state)
            else:
                result = {
                    "operation": operation,
                    "status": "skipped",
                    "durationMs": 0.0,
                    "at": utc_now(),
                    "reason": "unknown_operation",
                }
        except Exception as exc:  # noqa: BLE001 - harness must record and continue.
            result = {
                "operation": operation,
                "status": "error",
                "durationMs": 0.0,
                "at": utc_now(),
                "error": type(exc).__name__,
                "message": str(exc)[:500],
            }
        with result_lock:
            results.append(result)
        if think_time > 0:
            time.sleep(think_time)


def summarize_metrics(samples: list[dict[str, Any]]) -> dict[str, Any]:
    families: set[str] = set()
    gauge_series: dict[str, list[float]] = {}
    optional_present: set[str] = set()
    for sample in samples:
        metrics = sample.get("metrics", {})
        families.update(metrics.get("families", []))
        for name, value in metrics.get("gauges", {}).items():
            gauge_series.setdefault(name, []).append(float(value))
            if name in OPTIONAL_LEAK_PROXY_METRICS:
                optional_present.add(name)
        for item in metrics.get("samples", []):
            name = item.get("name")
            if name in OPTIONAL_LEAK_PROXY_METRICS:
                optional_present.add(str(name))
    gauge_summary = {}
    for name, values in gauge_series.items():
        if not values:
            continue
        monotonic = all(values[index] >= values[index - 1] for index in range(1, len(values)))
        gauge_summary[name] = {
            "samples": len(values),
            "first": values[0],
            "last": values[-1],
            "min": min(values),
            "max": max(values),
            "growth": values[-1] - values[0],
            "monotonicNonDecreasing": monotonic,
        }
    unavailable = [
        name
        for name in OPTIONAL_LEAK_PROXY_METRICS
        if name.startswith("markhand_") and name not in optional_present and name not in gauge_series
    ]
    return {
        "sampleCount": len(samples),
        "familiesObserved": sorted(families),
        "monitoredFamilies": MONITORED_MARKHAND_METRICS,
        "gauges": gauge_summary,
        "resourceLeakProxyMetricsUnavailable": unavailable,
        "readyFailures": sum(1 for item in samples if not item.get("ready", {}).get("ok")),
        "metricsFailures": sum(1 for item in samples if not item.get("metrics", {}).get("ok")),
    }


def summarize_operations(results: list[dict[str, Any]], wall_seconds: float) -> dict[str, Any]:
    by_op: dict[str, dict[str, Any]] = {}
    for operation in sorted({str(item.get("operation")) for item in results}):
        items = [item for item in results if item.get("operation") == operation]
        durations = [float(item.get("durationMs", 0.0)) for item in items if item.get("status") != "skipped"]
        successes = [item for item in items if item.get("status") == "ok"]
        errors = [item for item in items if item.get("status") == "error"]
        partials = [item for item in items if item.get("status") == "partial"]
        skipped = [item for item in items if item.get("status") == "skipped"]
        by_op[operation] = {
            "attempts": len(items),
            "successes": len(successes),
            "partials": len(partials),
            "errors": len(errors),
            "skipped": len(skipped),
            "durationMs": duration_stats(durations),
            "httpStatusCounts": status_counts(items),
        }
    ingest_successes = by_op.get("ingest", {}).get("successes", 0)
    wall_hours = wall_seconds / 3600.0 if wall_seconds > 0 else 0.0
    if "ingest" in by_op:
        by_op["ingest"]["successfulDocumentsPerHour"] = (
            round(float(ingest_successes) / wall_hours, 3) if wall_hours else 0.0
        )
    return by_op


def status_counts(items: list[dict[str, Any]]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for item in items:
        status = item.get("httpStatus")
        if status is None:
            response = item.get("response")
            if isinstance(response, dict):
                status = response.get("status")
        key = "none" if status is None else str(status)
        counts[key] = counts.get(key, 0) + 1
    return counts


def self_skip_payload(args: argparse.Namespace, profile: dict[str, Any], reason: str) -> dict[str, Any]:
    return {
        "version": 1,
        "reportId": "p1b-o05-mixed-load-soak",
        "generatedAt": utc_now(),
        "command": "python3 bench/markhand_web/soak/run_soak.py",
        "mode": "self-skip-no-live-target",
        "status": "skipped",
        "skipReason": reason,
        "profile": profile_metadata(args.profile, profile),
        "git": git_status(),
        "environment": {
            "environmentId": "current-runner-no-live-stack",
            "targetEnvironmentId": profile.get("targetEnvironmentId", "on-prem-reference"),
            "targetMatch": False,
        },
        "targetMatch": False,
        "targetResultsValidForGate": False,
        "implementationSha256": implementation_sha256(),
        "toolVersions": stack_versions(),
        "gateMappings": profile.get("gateMappings", []),
        "metrics": {
            "sampleCount": 0,
            "familiesObserved": [],
            "monitoredFamilies": MONITORED_MARKHAND_METRICS,
            "resourceLeakProxyMetricsUnavailable": OPTIONAL_LEAK_PROXY_METRICS,
        },
        "operations": {},
        "state": {"activeDocuments": 0, "deletedDocuments": 0, "observedJobs": 0},
        "failureInjections": failure_injection_plan(profile, []),
        "doesNotClaim": [
            "G0-SLO-QUERY-P95 pass",
            "G0-SLO-QUERY-P99 pass",
            "G0-CAP-INGEST-THROUGHPUT pass",
            "soak stability pass",
            "Profile B evidence",
        ],
        "notes": [
            "No target URL/token was configured, so no live load was executed.",
            "Run against F02 compose.poc.yml or real on-prem-reference infrastructure to produce numeric evidence.",
            DOES_NOT_CLAIM,
        ],
    }


def failure_injection_plan(profile: dict[str, Any], executed: list[str]) -> list[dict[str, Any]]:
    executed_set = set(executed)
    planned = []
    for item in profile.get("failureInjections", []):
        if not isinstance(item, dict):
            continue
        planned.append(
            {
                "id": item.get("id"),
                "mode": item.get("mode", "documented-manual-step"),
                "command": item.get("command"),
                "restoreCommand": item.get("restoreCommand"),
                "expectedSignal": item.get("expectedSignal"),
                "executedByHarness": False,
                "operatorRecordedExecuted": item.get("id") in executed_set,
            }
        )
    return planned


def profile_metadata(profile_path: Path, profile: dict[str, Any]) -> dict[str, Any]:
    return {
        "path": relative(profile_path),
        "sha256": file_sha256(profile_path),
        "profileId": profile.get("profileId"),
        "durationSeconds": profile.get("durationSeconds"),
        "rampSeconds": profile.get("rampSeconds"),
        "concurrency": profile.get("concurrency"),
        "operationWeights": profile.get("operationWeights"),
        "targetRatesPerHour": profile.get("targetRatesPerHour"),
        "sampleIntervalSeconds": profile.get("sampleIntervalSeconds"),
    }


def build_payload(args: argparse.Namespace) -> dict[str, Any]:
    profile = load_json(args.profile)
    base_url = args.base_url or os.environ.get(str(profile.get("api", {}).get("baseUrlEnv", "MARKHAND_BASE_URL")))
    token = args.bearer_token or os.environ.get(
        str(profile.get("api", {}).get("bearerTokenEnv", "MARKHAND_BEARER_TOKEN"))
    )
    if not base_url:
        return self_skip_payload(args, profile, "target URL unset; pass --base-url or MARKHAND_BASE_URL")
    if not token:
        return self_skip_payload(args, profile, "bearer token unset; pass --bearer-token or MARKHAND_BEARER_TOKEN")

    duration = float(args.duration_seconds or profile.get("durationSeconds", 60))
    concurrency = int(args.concurrency or profile.get("concurrency", 1))
    if duration <= 0 or concurrency <= 0:
        raise HarnessError("duration and concurrency must be positive")

    client = ApiClient(base_url, token, float(profile.get("operationTimeoutSeconds", 30)))
    ready = client.get("/api/v1/health/ready")
    if not ready["ok"] and not args.allow_unready:
        raise HarnessError(
            f"target not ready status={ready['status']}; pass --allow-unready only for failure-injection windows"
        )
    collection_env = str(profile.get("api", {}).get("collectionIdEnv", "MARKHAND_COLLECTION_ID"))
    collection_id = args.collection_id or os.environ.get(collection_env)
    collection_id = ensure_collection(client, collection_id)

    state = SharedState()
    samples: list[dict[str, Any]] = []
    stop = threading.Event()
    sample_thread = threading.Thread(
        target=sampler,
        args=(client, float(profile.get("sampleIntervalSeconds", 5)), stop, samples),
        daemon=True,
    )
    sample_thread.start()

    results: list[dict[str, Any]] = []
    result_lock = threading.Lock()
    started = time.monotonic()
    deadline = started + duration
    with concurrent.futures.ThreadPoolExecutor(max_workers=concurrency) as executor:
        futures = [
            executor.submit(
                run_worker,
                index,
                client,
                profile,
                collection_id,
                state,
                deadline,
                float(profile.get("rampSeconds", 0)),
                results,
                result_lock,
            )
            for index in range(concurrency)
        ]
        for future in futures:
            future.result()
    stop.set()
    sample_thread.join(timeout=5)
    wall_seconds = time.monotonic() - started
    operations = summarize_operations(results, wall_seconds)
    metrics = summarize_metrics(samples)
    total_errors = sum(int(value.get("errors", 0)) for value in operations.values())
    target_match = bool(args.target_match)

    return {
        "version": 1,
        "reportId": "p1b-o05-mixed-load-soak",
        "generatedAt": utc_now(),
        "command": "python3 bench/markhand_web/soak/run_soak.py",
        "mode": "live-mixed-load-soak",
        "status": "completed",
        "profile": profile_metadata(args.profile, profile),
        "git": git_status(),
        "environment": {
            "environmentId": args.environment_id,
            "targetEnvironmentId": profile.get("targetEnvironmentId", "on-prem-reference"),
            "targetMatch": target_match,
            "baseUrl": base_url,
            "collectionId": collection_id,
        },
        "targetMatch": target_match,
        "targetResultsValidForGate": target_match,
        "implementationSha256": implementation_sha256(),
        "toolVersions": stack_versions(),
        "durationSecondsObserved": round(wall_seconds, 3),
        "operations": operations,
        "operationSamples": results[-5000:],
        "metrics": metrics,
        "metricSamples": samples[-2000:],
        "state": state.snapshot(),
        "gateMappings": profile.get("gateMappings", []),
        "failureInjections": failure_injection_plan(profile, args.executed_failure_step),
        "restoreLeg": profile.get("restoreLeg"),
        "soakQualification": {
            "completedDurationSeconds": round(wall_seconds, 3),
            "operationErrors": total_errors,
            "readyFailures": metrics["readyFailures"],
            "metricsFailures": metrics["metricsFailures"],
            "noUnboundedLeakClaimed": False,
            "numericGateValid": target_match,
        },
        "doesNotClaim": [] if target_match else [
            "G0-SLO-QUERY-P95 pass",
            "G0-SLO-QUERY-P99 pass",
            "G0-CAP-INGEST-THROUGHPUT pass",
            "soak stability pass",
            "Profile B evidence",
        ],
        "notes": [
            "Live run completed; numeric gate validity still requires targetMatch=true on the approved environment.",
            "The public HTTP API has no reconcile route; the reconcile leg uses document reindex/readiness probes and observes worker reconcile through metrics/runbook steps.",
            DOES_NOT_CLAIM if not target_match else "targetMatch=true was asserted by the operator; verify environment fingerprint before gate use.",
        ],
    }


def render_report(payload: dict[str, Any]) -> str:
    profile = payload["profile"]
    lines = [
        "# Phase-1B mixed-load soak",
        "",
        f"- Generated: `{payload['generatedAt']}`",
        f"- Mode: `{payload['mode']}`",
        f"- Status: `{payload['status']}`",
        f"- Profile: `{profile.get('profileId')}`",
        f"- Git commit: `{payload['git']['commit']}`",
        f"- Dirty at harness start: `{str(payload['git']['dirty']).lower()}`",
        f"- `targetMatch`: `{str(payload['targetMatch']).lower()}`",
        f"- `targetResultsValidForGate`: `{str(payload['targetResultsValidForGate']).lower()}`",
        "",
        "## Caveat",
        "",
        "Numeric G0-SLO/G0-CAP/soak gates require sustained real infrastructure. ",
        "This sandbox does not provide that infrastructure; self-skip or targetMatch=false output is pending evidence only.",
        "",
        "## Operation mix",
        "",
        f"- Duration seconds: `{profile.get('durationSeconds')}`",
        f"- Ramp seconds: `{profile.get('rampSeconds')}`",
        f"- Concurrency: `{profile.get('concurrency')}`",
        f"- Operation weights: `{json.dumps(profile.get('operationWeights'), sort_keys=True)}`",
        "",
        "| operation | attempts | ok | partial | error | skipped | p95 ms | p99 ms |",
        "|---|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for name, stats in sorted(payload.get("operations", {}).items()):
        duration = stats.get("durationMs", {})
        lines.append(
            "| "
            f"{name} | {stats.get('attempts', 0)} | {stats.get('successes', 0)} | "
            f"{stats.get('partials', 0)} | {stats.get('errors', 0)} | {stats.get('skipped', 0)} | "
            f"{duration.get('p95')} | {duration.get('p99')} |"
        )
    metrics = payload.get("metrics", {})
    lines.extend(
        [
            "",
            "## Monitored metrics",
            "",
            f"- Samples: `{metrics.get('sampleCount', 0)}`",
            f"- Ready failures: `{metrics.get('readyFailures', 0)}`",
            f"- Metrics scrape failures: `{metrics.get('metricsFailures', 0)}`",
            f"- Families observed: `{', '.join(metrics.get('familiesObserved', [])) or 'none'}`",
            "",
            "Queue/leak proxy gauges:",
            "",
            "| metric | samples | first | last | min | max | growth | monotonic |",
            "|---|---:|---:|---:|---:|---:|---:|---|",
        ]
    )
    for name, gauge in sorted(metrics.get("gauges", {}).items()):
        lines.append(
            "| "
            f"`{name}` | {gauge['samples']} | {gauge['first']} | {gauge['last']} | "
            f"{gauge['min']} | {gauge['max']} | {gauge['growth']} | "
            f"{str(gauge['monotonicNonDecreasing']).lower()} |"
        )
    if not metrics.get("gauges"):
        lines.append("| none | 0 |  |  |  |  |  |  |")
    lines.extend(
        [
            "",
            "Unavailable optional leak proxies:",
            "",
        ]
    )
    for name in metrics.get("resourceLeakProxyMetricsUnavailable", []):
        lines.append(f"- `{name}`")
    lines.extend(
        [
            "",
            "## Gate metric mapping",
            "",
            "| gate id | soak metric |",
            "|---|---|",
        ]
    )
    for item in payload.get("gateMappings", []):
        lines.append(f"| `{item.get('gateId')}` | `{item.get('soakMetric')}` |")
    lines.extend(
        [
            "",
            "## Failure-injection plan",
            "",
            "| id | executed by harness | operator recorded executed | expected signal |",
            "|---|---|---|---|",
        ]
    )
    for item in payload.get("failureInjections", []):
        lines.append(
            "| "
            f"`{item.get('id')}` | `{str(item.get('executedByHarness')).lower()}` | "
            f"`{str(item.get('operatorRecordedExecuted')).lower()}` | {item.get('expectedSignal')} |"
        )
    if payload.get("notes"):
        lines.extend(["", "## Notes", ""])
        for note in payload["notes"]:
            lines.append(f"- {note}")
    if payload["git"]["dirtyPaths"]:
        lines.extend(["", "Dirty paths at harness start:"])
        for path in payload["git"]["dirtyPaths"][:20]:
            lines.append(f"- `{path}`")
    lines.append("")
    return "\n".join(lines)


def write_outputs(payload: dict[str, Any], summary: Path, report: Path) -> None:
    summary.parent.mkdir(parents=True, exist_ok=True)
    report.parent.mkdir(parents=True, exist_ok=True)
    summary.write_text(json.dumps(payload, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    report.write_text(render_report(payload), encoding="utf-8")


def self_test() -> None:
    metrics = parse_prometheus(
        """
# HELP markhand_jobs_queue_depth Background jobs waiting.
markhand_jobs_queue_depth 4
markhand_jobs_in_flight 2
markhand_http_requests_total{route="/api/v1/search",method="POST",status="200"} 7
"""
    )
    assert "markhand_jobs_queue_depth" in metrics["families"]
    assert metrics["gauges"]["markhand_jobs_queue_depth"] == 4.0
    assert percentile([1, 2, 3], 0.95) == 2.9
    profile = load_json(DEFAULT_PROFILE)
    payload = self_skip_payload(
        argparse.Namespace(profile=DEFAULT_PROFILE),
        profile,
        "test",
    )
    assert payload["targetMatch"] is False
    assert payload["status"] == "skipped"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", type=Path, default=DEFAULT_PROFILE)
    parser.add_argument("--summary", type=Path, default=SUMMARY_PATH)
    parser.add_argument("--report", type=Path, default=REPORT_PATH)
    parser.add_argument("--base-url", default=None)
    parser.add_argument("--bearer-token", default=None)
    parser.add_argument("--collection-id", default=None)
    parser.add_argument("--duration-seconds", type=float, default=None)
    parser.add_argument("--concurrency", type=int, default=None)
    parser.add_argument("--environment-id", default="current-runner-or-operator-specified")
    parser.add_argument("--target-match", action="store_true")
    parser.add_argument("--allow-unready", action="store_true")
    parser.add_argument("--executed-failure-step", action="append", default=[])
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    args.profile = args.profile.resolve()
    args.summary = args.summary.resolve()
    args.report = args.report.resolve()

    if args.self_test:
        self_test()
        print("self-test ok")
        return 0

    try:
        payload = build_payload(args)
        write_outputs(payload, args.summary, args.report)
    except HarnessError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    print(f"wrote {relative(args.summary)}")
    print(f"wrote {relative(args.report)}")
    print(f"status={payload['status']}")
    print(f"targetMatch={str(payload['targetMatch']).lower()}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
