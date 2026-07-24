"""Deterministic CPU-only OpenAI-compatible inference stub for local development.

Returns L2-normalized, non-zero vectors. The Markhand server rejects zero and
non-normalized embeddings (see fileconv_server::services::embedding). A minimal
chat endpoint exercises provider telemetry while grounded answers still follow
the server's fail-closed extractive policy.
"""

from __future__ import annotations

import hashlib
import json
import math
import os
from http.server import BaseHTTPRequestHandler, HTTPServer


DIMENSIONS = int(os.environ.get("MARKHAND_MOCK_EMBEDDING_DIMENSIONS", "8"))


def l2_normalize(values: list[float]) -> list[float]:
    norm = math.sqrt(sum(value * value for value in values))
    if not math.isfinite(norm) or norm <= 0.0:
        raise ValueError("cannot L2-normalize empty/zero embedding")
    return [value / norm for value in values]


def embedding_for(text: str, dimensions: int) -> list[float]:
    """Deterministic unit vector derived from the input text."""
    digest = hashlib.sha256(text.encode("utf-8")).digest()
    values: list[float] = []
    counter = 0
    while len(values) < dimensions:
        block = hashlib.sha256(digest + counter.to_bytes(4, "big")).digest()
        for offset in range(0, len(block), 4):
            if len(values) >= dimensions:
                break
            # Map bytes to (-1, 1) so the vector is never the zero vector.
            raw = int.from_bytes(block[offset : offset + 4], "big")
            values.append((raw / 4294967295.0) * 2.0 - 1.0)
        counter += 1
    return l2_normalize(values)


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/health":
            self._send(200, {"status": "ok"})
            return
        self._send(404, {"error": {"code": "not_found"}})

    def do_POST(self):
        if self.path not in ("/v1/embeddings", "/v1/chat/completions"):
            self._send(404, {"error": {"code": "not_found"}})
            return
        length = int(self.headers.get("Content-Length", "0"))
        payload = json.loads(self.rfile.read(length) or b"{}")
        if self.path == "/v1/chat/completions":
            self._send(
                200,
                {
                    "id": "markhand-mock-chat",
                    "object": "chat.completion",
                    "model": payload.get("model", "markhand-mock-chat"),
                    "choices": [
                        {
                            "index": 0,
                            "message": {
                                "role": "assistant",
                                "content": "Câu trả lời kiểm thử có căn cứ [1].",
                            },
                            "finish_reason": "stop",
                        }
                    ],
                    "usage": {
                        "prompt_tokens": 0,
                        "completion_tokens": 0,
                        "total_tokens": 0,
                    },
                },
            )
            return
        inputs = payload.get("input", [])
        if isinstance(inputs, str):
            inputs = [inputs]
        data = [
            {
                "object": "embedding",
                "index": index,
                "embedding": embedding_for(str(text), DIMENSIONS),
            }
            for index, text in enumerate(inputs)
        ]
        self._send(
            200,
            {
                "object": "list",
                "data": data,
                "model": payload.get("model", "markhand-mock"),
                "usage": {"prompt_tokens": 0, "total_tokens": 0},
            },
        )

    def log_message(self, _format, *_args):
        return

    def _send(self, status, body):
        content = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(content)))
        self.end_headers()
        self.wfile.write(content)


if __name__ == "__main__":
    HTTPServer(("0.0.0.0", 8080), Handler).serve_forever()
