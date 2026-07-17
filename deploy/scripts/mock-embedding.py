"""Deterministic CPU-only OpenAI-compatible embedding stub for local development."""

import json
import os
from http.server import BaseHTTPRequestHandler, HTTPServer


DIMENSIONS = int(os.environ.get("MARKHAND_MOCK_EMBEDDING_DIMENSIONS", "8"))


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/health":
            self._send(200, {"status": "ok"})
            return
        self._send(404, {"error": {"code": "not_found"}})

    def do_POST(self):
        if self.path != "/v1/embeddings":
            self._send(404, {"error": {"code": "not_found"}})
            return
        length = int(self.headers.get("Content-Length", "0"))
        payload = json.loads(self.rfile.read(length) or b"{}")
        inputs = payload.get("input", [])
        if isinstance(inputs, str):
            inputs = [inputs]
        data = [
            {"object": "embedding", "index": index, "embedding": [0.0] * DIMENSIONS}
            for index, _ in enumerate(inputs)
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


HTTPServer(("0.0.0.0", 8080), Handler).serve_forever()
