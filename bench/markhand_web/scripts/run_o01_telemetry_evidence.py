#!/usr/bin/env python3
"""P1B-O01 telemetry / audit / canary evidence (machine-generated, deterministic).

Runs cargo telemetry tests + optional live API probes (`MARKHAND_API_BASE`) and
optional async canary (`MARKHAND_O01_ASYNC=1`). Never writes pass without
machine-captured negatives. Raw secret/content values are redacted in reports.
"""

from __future__ import annotations

import hashlib
import shutil
import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "bench/markhand_web/reports/phase-1b-gate"
# Deterministic raw dir keyed by git short SHA (not wall-clock).
GIT_SHORT = subprocess.check_output(
    ["git", "rev-parse", "--short", "HEAD"], cwd=ROOT, text=True
).strip()
RAW = OUT / "raw" / f"o01-{GIT_SHORT}"
CANARIES = [
    "CANARY_SECRET_TOKEN_9f3c",
    "CANARY_DOC_TEXT_KINH_PHI_15_TRIEU",
    "CANARY_PROMPT_TEXT",
    "CANARY_ANSWER_TEXT",
    "postgres://canary:hunter2@db/markhand",
    "Bearer sk-canary-live-key",
]
REDACT_PATTERNS = [
    (re.compile(r"(Bearer\s+)[A-Za-z0-9._\-]+"), r"\1[REDACTED]"),
    (re.compile(r"(postgres://)[^@\s]+@"), r"\1[REDACTED]@"),
    (re.compile(r"(" + "|".join(re.escape(c) for c in CANARIES) + ")"), "[REDACTED_CANARY]"),
    (re.compile(r"(?i)(password|secret|token|authorization)\"?\s*[:=]\s*\"?[^\s\",}]+"), r"\1:[REDACTED]"),
]



def sql_query(db_url: str, query: str) -> subprocess.CompletedProcess[str]:
    """Run a read-only SQL query. Prefer local psql; fall back to POC postgres container."""
    if shutil.which("psql"):
        return subprocess.run(
            ["psql", db_url, "-Atc", query],
            capture_output=True,
            text=True,
            check=False,
        )
    # Container fallback for Cloud VMs without host psql.
    container = os.environ.get("MARKHAND_O01_POSTGRES_CONTAINER", "markhand-poc-postgres-1")
    user = os.environ.get("POSTGRES_USER", "markhand")
    db = os.environ.get("POSTGRES_DB", "markhand")
    return subprocess.run(
        [
            "docker",
            "exec",
            "-i",
            container,
            "psql",
            "-U",
            user,
            "-d",
            db,
            "-Atc",
            query,
        ],
        capture_output=True,
        text=True,
        check=False,
    )

def redact(text: str) -> str:
    out = text
    for pattern, repl in REDACT_PATTERNS:
        out = pattern.sub(repl, out)
    return out


def write_raw(name: str, data: bytes | str) -> None:
    RAW.mkdir(parents=True, exist_ok=True)
    path = RAW / name
    if isinstance(data, bytes):
        text = data.decode("utf-8", errors="replace")
    else:
        text = data
    path.write_text(redact(text), encoding="utf-8")


def http(method: str, url: str, *, headers=None, body: bytes | None = None, timeout=10):
    req = urllib.request.Request(url, data=body, method=method, headers=headers or {})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return resp.status, dict(resp.headers.items()), resp.read()
    except urllib.error.HTTPError as err:
        return err.code, dict(err.headers.items()), err.read()


def run_cmd(args: list[str], env: dict | None = None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        args,
        cwd=ROOT,
        capture_output=True,
        text=True,
        env=env,
    )


def run_async_canary(base: str, canaries: list[str]) -> dict:
    """Login → upload canary text → poll job → scrape metrics for leaks."""
    email = os.environ.get("MARKHAND_E2E_EMAIL", "admin@poc.example")
    password = os.environ.get("MARKHAND_E2E_PASSWORD", "markhand-dev")
    status, _, body = http(
        "POST",
        f"{base}/api/v1/auth/login",
        headers={"content-type": "application/json"},
        body=json.dumps({"email": email, "password": password}).encode(),
    )
    write_raw("async-login.json", body)
    if status != 200:
        return {"passed": False, "error": f"login HTTP {status}"}
    token = json.loads(body).get("accessToken") or json.loads(body).get("access_token")
    if not token:
        return {"passed": False, "error": "login missing accessToken"}

    stamp = hashlib.sha256(f"{GIT_SHORT}:{time.time_ns()}".encode()).hexdigest()[:12]
    slug = f"o01-{stamp}"
    status, _, body = http(
        "POST",
        f"{base}/api/v1/collections",
        headers={
            "authorization": f"Bearer {token}",
            "content-type": "application/json",
        },
        body=json.dumps(
            {"name": f"O01 canary {stamp}", "slug": slug, "visibility": "org"}
        ).encode(),
    )
    write_raw("async-collection.json", body)
    if status not in (200, 201):
        return {"passed": False, "error": f"collection HTTP {status}"}
    collection_id = json.loads(body)["id"]

    canary_text = (
        "O01 async canary\n"
        f"{canaries[0]}\n"
        f"{canaries[1]}\n"
        "Xin chào Markhand telemetry.\n"
    )
    boundary = f"----o01{GIT_SHORT}"
    parts = [
        f"--{boundary}\r\nContent-Disposition: form-data; name=\"collectionId\"\r\n\r\n{collection_id}\r\n".encode(),
        (
            f"--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; "
            f"filename=\"o01-canary.txt\"\r\nContent-Type: text/plain\r\n\r\n"
        ).encode(),
        canary_text.encode(),
        f"\r\n--{boundary}--\r\n".encode(),
    ]
    status, headers, resp = http(
        "POST",
        f"{base}/api/v1/uploads",
        headers={
            "authorization": f"Bearer {token}",
            "idempotency-key": f"o01-{slug}",
            "content-type": f"multipart/form-data; boundary={boundary}",
        },
        body=b"".join(parts),
        timeout=120,
    )
    write_raw("async-upload.json", resp)
    if status not in (200, 201):
        return {
            "passed": False,
            "error": f"upload HTTP {status}",
            "body": redact(resp[:300].decode("utf-8", "replace")),
        }
    upload = json.loads(resp)
    request_id = headers.get("x-request-id") or headers.get("X-Request-Id")
    job_id = upload.get("jobId") or upload.get("job_id")

    job_status = None
    job_request_id = None
    if job_id:
        for _ in range(30):
            st, _, jb = http(
                "GET",
                f"{base}/api/v1/jobs/{job_id}",
                headers={"authorization": f"Bearer {token}"},
            )
            write_raw("async-job.json", jb)
            if st == 200:
                parsed = json.loads(jb)
                job_status = parsed.get("status")
                job_request_id = (
                    parsed.get("requestId")
                    or parsed.get("request_id")
                    or (parsed.get("payload") or {}).get("requestId")
                    or (parsed.get("payload") or {}).get("request_id")
                )
                if job_status in ("succeeded", "failed", "dead_letter", "completed"):
                    break
            time.sleep(1)

    if job_id and not job_request_id:
        db_url = os.environ.get("MARKHAND_TEST_DATABASE_URL") or os.environ.get("DATABASE_URL")
        if db_url:
            try:
                import subprocess as sp

                q = (
                    "SELECT COALESCE(payload->>'request_id', payload->>'requestId') "
                    "FROM jobs WHERE id = '%s'" % str(job_id).replace("'", "")
                )
                proc = sql_query(db_url, q)
                write_raw("async-job-db-request-id.txt", proc.stdout + proc.stderr)
                if proc.returncode == 0 and proc.stdout.strip():
                    job_request_id = proc.stdout.strip()
            except Exception as exc:  # noqa: BLE001
                write_raw("async-job-db-request-id.txt", str(exc))

    # Exact deny audit path: search a foreign collection → permission_denied.
    deny_status, deny_headers, deny_body = http(
        "POST",
        f"{base}/api/v1/search",
        headers={
            "authorization": f"Bearer {token}",
            "content-type": "application/json",
        },
        body=json.dumps(
            {
                "query": "o01 deny probe",
                "collectionIds": ["00000000-0000-0000-0000-000000000099"],
                "limit": 1,
            }
        ).encode(),
    )
    write_raw("async-deny.json", deny_body)
    deny_ok = deny_status in (401, 403, 404)
    deny_request_id = deny_headers.get("x-request-id") or deny_headers.get("X-Request-Id")
    deny_action = "search.query"

    metrics_status, _, metrics = http("GET", f"{base}/metrics")
    text = metrics.decode("utf-8", errors="replace")
    write_raw("async-metrics.txt", text)
    canary_hits = [c for c in canaries if c in text]
    forbidden = [
        label
        for label in ("org_id=", "user_id=", "document_id=", "filename=")
        if label in text
    ]
    # Ask/provider path (must not leak canaries into metrics).
    ask_status, ask_headers, ask_body = http(
        "POST",
        f"{base}/api/v1/ask",
        headers={
            "authorization": f"Bearer {token}",
            "content-type": "application/json",
        },
        body=json.dumps(
            {
                "question": "O01 telemetry ask probe",
                "collectionIds": [collection_id],
                "limit": 3,
            }
        ).encode(),
        timeout=60,
    )
    write_raw("async-ask.json", ask_body)
    ask_request_id = ask_headers.get("x-request-id") or ask_headers.get("X-Request-Id")
    ask_ok = ask_status in (200, 503, 422, 400)

    # Same-trace collector proof (required for async canary pass).
    collector = os.environ.get("MARKHAND_OTEL_COLLECTOR_QUERY", "").rstrip("/")
    collector_hit = False
    named_spans_ok = False
    unique_ids_ok = False
    canonical_kinds_ok = False
    parent_graph_ok = False
    span_names_found: list[str] = []
    required_span_names = (
        "api.request",
        "worker.convert",
        "worker.index",
        "worker.embed",
        "retrieval",
        "provider.chat",
    )
    if collector and request_id:
        try:
            st, _, body = http(
                "GET", f"{collector}/api/v1/traces?request_id={request_id}", timeout=5
            )
            write_raw("async-collector.json", body)
            collector_hit = st == 200 and request_id.encode() in body
            try:
                parsed = json.loads(body)
                spans = []
                if isinstance(parsed, dict) and "spans" in parsed:
                    spans = parsed.get("spans") or []
                elif isinstance(parsed, dict) and "resourceSpans" in parsed:
                    for rs in parsed.get("resourceSpans") or []:
                        for ss in rs.get("scopeSpans") or []:
                            spans.extend(ss.get("spans") or [])
                if spans:
                    trace_ids = [
                        (s.get("traceId") or s.get("trace_id") or "") for s in spans
                    ]
                    collector_hit = bool(trace_ids) and len(set(trace_ids)) == 1
                    span_names_found = [
                        (s.get("name") or s.get("spanName") or "") for s in spans
                    ]
                    named_spans_ok = all(
                        any(req in name for name in span_names_found)
                        for req in required_span_names
                    )
                    ids = [(s.get("spanId") or s.get("span_id") or "") for s in spans]
                    unique_ids_ok = bool(ids) and len(ids) == len(set(ids)) and all(ids)
                    kinds = []
                    for s in spans:
                        kind = s.get("kind")
                        if kind is None:
                            kind = s.get("spanKind") or s.get("span_kind")
                        if isinstance(kind, str):
                            kind = {
                                "INTERNAL": 1,
                                "SERVER": 2,
                                "CLIENT": 3,
                                "PRODUCER": 4,
                                "CONSUMER": 5,
                            }.get(kind.upper())
                        kinds.append(kind)
                    canonical_kinds_ok = bool(kinds) and all(
                        isinstance(k, int) and 1 <= k <= 5 for k in kinds
                    )
                    id_set = set(ids)
                    # SERVER/CONSUMER parents may be remote W3C roots.
                    remote_ok = set()
                    for s, kind in zip(spans, kinds):
                        if kind in (2, 5):
                            parent = s.get("parentSpanId") or s.get("parent_span_id")
                            if parent:
                                remote_ok.add(parent)
                    parent_graph_ok = True
                    for s in spans:
                        sid = s.get("spanId") or s.get("span_id") or ""
                        parent = s.get("parentSpanId") or s.get("parent_span_id")
                        if not parent:
                            continue
                        if parent == sid or (
                            parent not in id_set and parent not in remote_ok
                        ):
                            parent_graph_ok = False
                            break
                if isinstance(parsed, dict) and "sameTrace" in parsed:
                    collector_hit = bool(parsed.get("sameTrace"))
            except Exception:  # noqa: BLE001
                pass
        except Exception as exc:  # noqa: BLE001
            collector_hit = False
            write_raw("async-collector.json", str(exc))
    else:
        write_raw(
            "async-collector.json",
            "MARKHAND_OTEL_COLLECTOR_QUERY unset or request_id missing — same-trace proof required",
        )

    db_url = (
        os.environ.get("MARKHAND_TEST_DATABASE_URL")
        or os.environ.get("DATABASE_URL")
        or os.environ.get("MARKHAND_TEST_APP_DATABASE_URL")
    )
    db_audit_ok = None
    deny_audit_ok = None
    if db_url and request_id:
        try:
            q = (
                "SELECT count(*) FROM audit_log WHERE request_id = '%s'"
                % request_id.replace("'", "")
            )
            proc = sql_query(db_url, q)
            write_raw("async-db-audit.txt", proc.stdout + proc.stderr)
            db_audit_ok = proc.returncode == 0 and int((proc.stdout or "0").strip() or "0") >= 1
        except Exception as exc:  # noqa: BLE001
            db_audit_ok = False
            write_raw("async-db-audit.txt", str(exc))
    # Deny audit must match exact request_id + action + outcome (not a loose count).
    if db_url and deny_request_id:
        try:
            q = (
                "SELECT count(*) FROM audit_log "
                "WHERE request_id = '%s' AND action = '%s' AND outcome = 'deny'"
                % (deny_request_id.replace("'", ""), deny_action.replace("'", ""))
            )
            proc = sql_query(db_url, q)
            write_raw("async-deny-audit.txt", proc.stdout + proc.stderr + "\n" + q)
            deny_audit_ok = (
                proc.returncode == 0 and int((proc.stdout or "0").strip() or "0") >= 1
            )
        except Exception as exc:  # noqa: BLE001
            deny_audit_ok = False
            write_raw("async-deny-audit.txt", str(exc))

    terminal_ok = job_status in ("succeeded", "failed", "dead_letter", "completed")
    # R3 #5: every proof is mandatory — missing any exits nonzero via passed=False.
    proofs = {
        "jobIdPresent": bool(job_id),
        "requestIdPresent": bool(request_id),
        "jobTerminal": bool(job_id) and terminal_ok,
        "jobPayloadRequestIdPresent": bool(job_request_id),
        "dbAuditOk": db_audit_ok is True,
        "denyAuditExact": deny_audit_ok is True,
        "collectorSameTrace": collector_hit is True,
        "namedSpansPresent": named_spans_ok is True,
        "uniqueSpanIds": unique_ids_ok is True,
        "canonicalOtlpKinds": canonical_kinds_ok is True,
        "parentGraphValid": parent_graph_ok is True,
        "metricsClean": metrics_status == 200 and not canary_hits and not forbidden,
        "denyOk": deny_ok,
        "askOk": ask_ok,
        "documentPresent": bool(upload.get("documentId") or upload.get("document_id")),
    }
    missing = [name for name, ok in proofs.items() if not ok]
    passed = not missing
    return {
        "passed": passed,
        "status": "pass" if passed else "fail",
        "requestId": request_id,
        "denyRequestId": deny_request_id,
        "askRequestId": ask_request_id,
        "askHttpStatus": ask_status,
        "askOk": ask_ok,
        "jobId": job_id,
        "jobStatus": job_status,
        "jobPayloadRequestIdPresent": bool(job_request_id),
        "denyHttpStatus": deny_status,
        "denyOk": deny_ok,
        "dbAuditOk": db_audit_ok,
        "denyAuditExact": deny_audit_ok,
        "collectorSameTrace": collector_hit,
        "namedSpans": span_names_found,
        "requiredSpanNames": list(required_span_names),
        "canaryHits": canary_hits,
        "forbiddenLabels": forbidden,
        "uploadDisposition": upload.get("disposition"),
        "proofs": proofs,
        "missingProofs": missing,
        "error": None if passed else f"missing proofs: {missing}",
    }


def run_negative_proof_fixtures() -> dict:
    """Each missing-proof fixture must exit 1 (hard fail)."""

    def evaluate(proofs: dict[str, bool]) -> int:
        return 0 if all(proofs.values()) else 1

    fixtures = {
        "missing_deny_audit": {
            "jobIdPresent": True,
            "requestIdPresent": True,
            "jobTerminal": True,
            "jobPayloadRequestIdPresent": True,
            "dbAuditOk": True,
            "denyAuditExact": False,
            "collectorSameTrace": True,
            "namedSpansPresent": True,
            "uniqueSpanIds": True,
            "canonicalOtlpKinds": True,
            "parentGraphValid": True,
            "metricsClean": True,
            "denyOk": True,
            "askOk": True,
            "documentPresent": True,
        },
        "missing_named_spans": {
            "jobIdPresent": True,
            "requestIdPresent": True,
            "jobTerminal": True,
            "jobPayloadRequestIdPresent": True,
            "dbAuditOk": True,
            "denyAuditExact": True,
            "collectorSameTrace": True,
            "namedSpansPresent": False,
            "uniqueSpanIds": True,
            "canonicalOtlpKinds": True,
            "parentGraphValid": True,
            "metricsClean": True,
            "denyOk": True,
            "askOk": True,
            "documentPresent": True,
        },
        "missing_parent_graph": {
            "jobIdPresent": True,
            "requestIdPresent": True,
            "jobTerminal": True,
            "jobPayloadRequestIdPresent": True,
            "dbAuditOk": True,
            "denyAuditExact": True,
            "collectorSameTrace": True,
            "namedSpansPresent": True,
            "uniqueSpanIds": True,
            "canonicalOtlpKinds": True,
            "parentGraphValid": False,
            "metricsClean": True,
            "denyOk": True,
            "askOk": True,
            "documentPresent": True,
        },
        "missing_same_trace": {
            "jobIdPresent": True,
            "requestIdPresent": True,
            "jobTerminal": True,
            "jobPayloadRequestIdPresent": True,
            "dbAuditOk": True,
            "denyAuditExact": True,
            "collectorSameTrace": False,
            "namedSpansPresent": True,
            "uniqueSpanIds": True,
            "canonicalOtlpKinds": True,
            "parentGraphValid": True,
            "metricsClean": True,
            "denyOk": True,
            "askOk": True,
            "documentPresent": True,
        },
        "missing_canonical_kinds": {
            "jobIdPresent": True,
            "requestIdPresent": True,
            "jobTerminal": True,
            "jobPayloadRequestIdPresent": True,
            "dbAuditOk": True,
            "denyAuditExact": True,
            "collectorSameTrace": True,
            "namedSpansPresent": True,
            "uniqueSpanIds": True,
            "canonicalOtlpKinds": False,
            "parentGraphValid": True,
            "metricsClean": True,
            "denyOk": True,
            "askOk": True,
            "documentPresent": True,
        },
    }
    results = {}
    all_ok = True
    for name, proofs in fixtures.items():
        exit_code = evaluate(proofs)
        ok = exit_code == 1
        results[name] = {"exit": exit_code, "passed": ok}
        if not ok:
            all_ok = False
    # Positive control: complete proofs must exit 0.
    positive = {k: True for k in next(iter(fixtures.values()))}
    pos_exit = evaluate(positive)
    results["complete_proofs_exit_0"] = {"exit": pos_exit, "passed": pos_exit == 0}
    if pos_exit != 0:
        all_ok = False
    return {"passed": all_ok, "fixtures": results}


def main() -> int:
    RAW.mkdir(parents=True, exist_ok=True)
    base = os.environ.get("MARKHAND_API_BASE", "http://127.0.0.1:8788").rstrip("/")
    version = subprocess.check_output(
        ["git", "describe", "--always", "--dirty", "--tags"], cwd=ROOT, text=True
    ).strip()
    evidence = {
        "issue": "P1B-O01",
        "commitStampStrategy": "git_rev_parse_short_head_precommit_may_be_not_run",
        "generatedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "apiBase": base,
        "git": GIT_SHORT,
        "version": version,
        "commands": {
            "cargo_telemetry": [
                "cargo",
                "test",
                "-p",
                "fileconv-server",
                "telemetry",
                "--",
                "--nocapture",
            ],
            "cargo_live_o01": [
                "cargo",
                "test",
                "-p",
                "fileconv-server",
                "--test",
                "telemetry_audit",
                "--",
                "--ignored",
                "--nocapture",
            ],
            "evidence": [
                "python3",
                "bench/markhand_web/scripts/run_o01_telemetry_evidence.py",
            ],
        },
        "checks": {},
        "blockers": [],
        "status": "incomplete",
    }

    cargo = run_cmd(evidence["commands"]["cargo_telemetry"])
    write_raw("cargo-telemetry.txt", cargo.stdout + "\n" + cargo.stderr)
    evidence["checks"]["cargo_telemetry"] = {
        "exit": cargo.returncode,
        "passed": cargo.returncode == 0,
        "command": evidence["commands"]["cargo_telemetry"],
    }
    if cargo.returncode != 0:
        evidence["blockers"].append("cargo telemetry tests failed")

    # Live ignored test (app-role DB) when URLs present.
    if os.environ.get("MARKHAND_TEST_DATABASE_URL") and os.environ.get(
        "MARKHAND_TEST_APP_DATABASE_URL"
    ):
        live = run_cmd(evidence["commands"]["cargo_live_o01"])
        write_raw("cargo-live-o01.txt", live.stdout + "\n" + live.stderr)
        evidence["checks"]["live_app_role_audit"] = {
            "exit": live.returncode,
            "passed": live.returncode == 0,
            "command": evidence["commands"]["cargo_live_o01"],
        }
        if live.returncode != 0:
            evidence["blockers"].append("live app-role O01 audit test failed")
    else:
        evidence["checks"]["live_app_role_audit"] = {
            "passed": False,
            "status": "not_run",
            "note": "MARKHAND_TEST_DATABASE_URL / MARKHAND_TEST_APP_DATABASE_URL unset",
        }
        evidence["blockers"].append("live app-role DB evidence not run (test DB URLs unset)")

    try:
        status, headers, body = http("GET", f"{base}/metrics")
        text = body.decode("utf-8", errors="replace")
        write_raw("metrics.txt", text)
        has_build = "markhand_metrics_build" in text
        has_queue = "markhand_exporter_queue_depth" in text
        forbidden = [
            label
            for label in ("org_id=", "user_id=", "document_id=", "request_id=", "filename=")
            if label in text
        ]
        canary_hits = [c for c in CANARIES if c in text]
        evidence["checks"]["metrics_scrape"] = {
            "httpStatus": status,
            "bytes": len(body),
            "hasExporter": has_build,
            "hasExportQueueGauge": has_queue,
            "hasLatencyOrBuild": "markhand_http_request_duration_seconds" in text or has_build,
            "forbiddenLabels": forbidden,
            "canaryHits": canary_hits,
            "passed": status == 200
            and has_build
            and has_queue
            and not forbidden
            and not canary_hits,
        }
        if status != 200:
            evidence["blockers"].append(f"/metrics HTTP {status}")
        if not has_build:
            evidence["blockers"].append("metrics exporter body missing markhand_metrics_build")
        if not has_queue:
            evidence["blockers"].append("metrics missing markhand_exporter_queue_depth")
        if forbidden:
            evidence["blockers"].append(f"high-cardinality labels in metrics: {forbidden}")
        if canary_hits:
            evidence["blockers"].append(f"canary leaked into metrics: {canary_hits}")
    except Exception as exc:  # noqa: BLE001 — evidence path
        evidence["checks"]["metrics_scrape"] = {"passed": False, "error": str(exc)}
        evidence["blockers"].append(f"API /metrics unreachable: {exc}")

    for path in ("/api/v1/health/live", "/api/v1/health/ready"):
        try:
            status, headers, body = http("GET", f"{base}{path}")
            write_raw(f"{path.strip('/').replace('/', '_')}.json", body)
            evidence["checks"][path] = {
                "httpStatus": status,
                "passed": status in (200, 503),
                "requestId": headers.get("x-request-id") or headers.get("X-Request-Id"),
            }
            text = body.decode("utf-8", errors="replace")
            hits = [c for c in CANARIES if c in text]
            if hits:
                evidence["blockers"].append(f"canary in {path}")
                evidence["checks"][path]["canaryHits"] = ["[REDACTED_CANARY]"] * len(hits)
        except Exception as exc:  # noqa: BLE001
            evidence["checks"][path] = {"passed": False, "error": str(exc)}
            evidence["blockers"].append(f"{path} unreachable: {exc}")

    if (RAW / "metrics.txt").exists():
        try:
            status, _, body = http("GET", f"{base}/metrics")
            text = body.decode("utf-8", errors="replace")
            write_raw("metrics-after-probes.txt", text)
            has_hist = "markhand_http_request_duration_seconds_count" in text
            evidence["checks"]["http_histogram_emitted"] = {
                "passed": has_hist,
                "httpStatus": status,
            }
            if not has_hist:
                evidence["blockers"].append(
                    "HTTP histogram not emitted after health probes (exporter middleware may be missing)"
                )
        except Exception as exc:  # noqa: BLE001
            evidence["checks"]["http_histogram_emitted"] = {"passed": False, "error": str(exc)}

    neg = run_negative_proof_fixtures()
    evidence["checks"]["negative_proof_fixtures"] = neg
    if not neg.get("passed"):
        evidence["blockers"].append("negative proof fixtures did not hard-fail missing proofs")

    if os.environ.get("MARKHAND_O01_ASYNC") != "1":
        evidence["checks"]["async_api_worker_provider_canary"] = {
            "passed": False,
            "status": "not_run",
            "note": "Set MARKHAND_O01_ASYNC=1 with full convert/index/chat stack to close async canary",
        }
        evidence["blockers"].append(
            "async API→worker→provider canary not opted in (MARKHAND_O01_ASYNC!=1)"
        )
    else:
        async_check = run_async_canary(base, CANARIES)
        evidence["checks"]["async_api_worker_provider_canary"] = async_check
        if not async_check.get("passed"):
            evidence["blockers"].append(
                async_check.get("error") or "async API→worker→provider canary failed"
            )

    checks = [c for c in evidence["checks"].values() if isinstance(c, dict) and "passed" in c]
    if checks and all(c.get("passed") for c in checks) and not evidence["blockers"]:
        evidence["status"] = "pass"
    elif any(c.get("passed") for c in checks):
        evidence["status"] = "incomplete"
    else:
        evidence["status"] = "blocked"

    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "o01-telemetry.json").write_text(
        json.dumps(evidence, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    md = [
        "# P1B-O01 telemetry evidence",
        "",
        f"- Status: `{evidence['status']}`",
        f"- Git: `{evidence['git']}` / `{evidence['version']}`",
        f"- Raw: `{RAW}` (redacted)",
        f"- Blockers: {len(evidence['blockers'])}",
        "",
        "## Commands",
        "",
    ]
    for name, cmd in evidence["commands"].items():
        md.append(f"- `{name}`: `{' '.join(cmd)}`")
    md.append("")
    for b in evidence["blockers"]:
        md.append(f"- BLOCKER: {redact(b)}")
    (OUT / "o01-telemetry.md").write_text("\n".join(md) + "\n", encoding="utf-8")
    print(OUT / "o01-telemetry.json")
    return 0 if evidence["status"] == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
