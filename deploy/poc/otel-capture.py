"""Bounded OTLP/HTTP capture service for the P1B-O01 live canary.

Accepts the same JSON payload as an OTLP collector at ``POST /v1/traces`` and
exposes only normalized span metadata at
``GET /api/v1/traces?request_id=<uuid>``. It never stores event bodies, document
text, prompts, credentials, or arbitrary attributes.
"""

from __future__ import annotations

import json
import os
import threading
from collections import deque
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse


MAX_BODY_BYTES = int(os.environ.get("MARKHAND_OTEL_CAPTURE_MAX_BODY_BYTES", "8388608"))
MAX_SPANS = int(os.environ.get("MARKHAND_OTEL_CAPTURE_MAX_SPANS", "20000"))
SPANS: deque[dict[str, object]] = deque(maxlen=MAX_SPANS)
LOCK = threading.Lock()


def _string_attribute(attributes: object, key: str) -> str | None:
    if not isinstance(attributes, list):
        return None
    for item in attributes:
        if not isinstance(item, dict) or item.get("key") != key:
            continue
        value = item.get("value")
        if not isinstance(value, dict):
            return None
        raw = value.get("stringValue")
        return raw if isinstance(raw, str) else None
    return None


def _normalized_spans(payload: object) -> list[dict[str, object]]:
    if not isinstance(payload, dict):
        return []
    normalized: list[dict[str, object]] = []
    for resource_span in payload.get("resourceSpans") or []:
        if not isinstance(resource_span, dict):
            continue
        resource = resource_span.get("resource") or {}
        service = _string_attribute(
            resource.get("attributes") if isinstance(resource, dict) else None,
            "service.name",
        )
        for scope_span in resource_span.get("scopeSpans") or []:
            if not isinstance(scope_span, dict):
                continue
            for span in scope_span.get("spans") or []:
                if not isinstance(span, dict):
                    continue
                trace_id = span.get("traceId")
                span_id = span.get("spanId")
                name = span.get("name")
                kind = span.get("kind")
                request_id = _string_attribute(span.get("attributes"), "request_id")
                if not (
                    isinstance(trace_id, str)
                    and isinstance(span_id, str)
                    and isinstance(name, str)
                    and isinstance(kind, int)
                    and isinstance(request_id, str)
                ):
                    continue
                item: dict[str, object] = {
                    "traceId": trace_id,
                    "spanId": span_id,
                    "name": name,
                    "kind": kind,
                    "requestId": request_id,
                    "serviceName": service or "unknown",
                    "startTimeUnixNano": str(span.get("startTimeUnixNano") or "0"),
                    "endTimeUnixNano": str(span.get("endTimeUnixNano") or "0"),
                }
                parent = span.get("parentSpanId")
                if isinstance(parent, str) and parent:
                    item["parentSpanId"] = parent
                normalized.append(item)
    return normalized


class Handler(BaseHTTPRequestHandler):
    server_version = "markhand-otel-capture/1"

    def do_GET(self) -> None:
        parsed = urlparse(self.path)
        if parsed.path == "/health":
            self._send(200, {"status": "ok"})
            return
        if parsed.path != "/api/v1/traces":
            self._send(404, {"error": "not_found"})
            return
        request_id = (parse_qs(parsed.query).get("request_id") or [""])[0]
        if not request_id:
            self._send(400, {"error": "request_id_required"})
            return
        with LOCK:
            snapshot = list(SPANS)
        trace_ids = {
            str(span["traceId"])
            for span in snapshot
            if span.get("requestId") == request_id
        }
        spans = [span for span in snapshot if str(span["traceId"]) in trace_ids]
        spans.sort(key=lambda span: (str(span["startTimeUnixNano"]), str(span["spanId"])))
        self._send(
            200,
            {
                "requestId": request_id,
                "sameTrace": len(trace_ids) == 1 and bool(spans),
                "spans": spans,
            },
        )

    def do_POST(self) -> None:
        if urlparse(self.path).path != "/v1/traces":
            self._send(404, {"error": "not_found"})
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
        except ValueError:
            self._send(400, {"error": "invalid_content_length"})
            return
        if length <= 0 or length > MAX_BODY_BYTES:
            self._send(413, {"error": "body_size_invalid"})
            return
        try:
            payload = json.loads(self.rfile.read(length))
        except (json.JSONDecodeError, UnicodeDecodeError):
            self._send(400, {"error": "invalid_json"})
            return
        spans = _normalized_spans(payload)
        with LOCK:
            SPANS.extend(spans)
        self._send(200, {"acceptedSpans": len(spans)})

    def log_message(self, _format: str, *_args: object) -> None:
        return

    def _send(self, status: int, body: object) -> None:
        content = json.dumps(body, separators=(",", ":")).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(content)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(content)


if __name__ == "__main__":
    server = ThreadingHTTPServer(("0.0.0.0", 4318), Handler)
    server.daemon_threads = True
    server.serve_forever()
