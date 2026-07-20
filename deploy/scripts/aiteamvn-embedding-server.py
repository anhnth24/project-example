"""OpenAI-compatible CPU embedding server for Markhand workers (AITeamVN P0-05 pin)."""

from __future__ import annotations

import json
import os
import threading
import unicodedata
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import numpy as np

HUB_ID = os.environ.get(
    "MARKHAND_EMBEDDING_HUB_ID", "AITeamVN/Vietnamese_Embedding"
)
REVISION = os.environ.get(
    "MARKHAND_EMBEDDING_REVISION", "dea33aa1ab339f38d66ae0a40e6c40e0a9249568"
)
MODEL_ID = os.environ.get("MARKHAND_EMBEDDING_MODEL_ID", HUB_ID)
DIMENSIONS = int(os.environ.get("MARKHAND_EMBEDDING_DIMENSIONS", "1024"))
MAX_SEQ_LENGTH = int(os.environ.get("MARKHAND_EMBEDDING_MAX_SEQ_LENGTH", "2048"))
BATCH_SIZE = max(1, int(os.environ.get("MARKHAND_EMBEDDING_BATCH_SIZE", "16")))
DEVICE = os.environ.get("MARKHAND_EMBEDDING_DEVICE", "cpu")
API_KEY = os.environ.get("MARKHAND_EMBEDDING_SERVER_API_KEY", "dev-embedding-key")
LISTEN_HOST = os.environ.get("MARKHAND_EMBEDDING_LISTEN_HOST", "0.0.0.0")
LISTEN_PORT = int(os.environ.get("MARKHAND_EMBEDDING_LISTEN_PORT", "8080"))

_model = None
_model_lock = threading.Lock()
_ready = False
_load_error: str | None = None


def l2_normalize(matrix: np.ndarray) -> np.ndarray:
    norms = np.linalg.norm(matrix, axis=1, keepdims=True)
    norms = np.maximum(norms, 1e-12)
    return matrix / norms


def prepare_text(text: str) -> str:
    return unicodedata.normalize("NFC", text)


def load_model() -> None:
    global _model, _ready, _load_error
    try:
        from sentence_transformers import SentenceTransformer

        model = SentenceTransformer(HUB_ID, revision=REVISION, device=DEVICE)
        model.max_seq_length = MAX_SEQ_LENGTH
        probe = model.encode(
            ["markhand embedding probe"],
            batch_size=1,
            show_progress_bar=False,
            convert_to_numpy=True,
            normalize_embeddings=False,
        )
        probe = l2_normalize(np.asarray(probe, dtype=np.float32))
        if probe.shape[1] != DIMENSIONS:
            raise RuntimeError(
                f"model returned dim={probe.shape[1]}, expected {DIMENSIONS}"
            )
        with _model_lock:
            _model = model
            _ready = True
            _load_error = None
    except Exception as error:  # noqa: BLE001 — surface load failure via /health
        with _model_lock:
            _model = None
            _ready = False
            _load_error = str(error)


def embed_batch(texts: list[str]) -> list[list[float]]:
    with _model_lock:
        if _model is None:
            raise RuntimeError(_load_error or "embedding model is not loaded")
        model = _model
    prepared = [prepare_text(text) for text in texts]
    vectors: list[np.ndarray] = []
    for offset in range(0, len(prepared), BATCH_SIZE):
        batch = prepared[offset : offset + BATCH_SIZE]
        encoded = model.encode(
            batch,
            batch_size=min(BATCH_SIZE, len(batch)),
            show_progress_bar=False,
            convert_to_numpy=True,
            normalize_embeddings=False,
        )
        vectors.append(l2_normalize(np.asarray(encoded, dtype=np.float32)))
    matrix = np.vstack(vectors) if vectors else np.empty((0, DIMENSIONS), dtype=np.float32)
    if matrix.shape[1] != DIMENSIONS:
        raise RuntimeError(
            f"batch returned dim={matrix.shape[1]}, expected {DIMENSIONS}"
        )
    return matrix.tolist()


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/health":
            if _ready:
                self._send(
                    200,
                    {
                        "status": "ok",
                        "ready": True,
                        "model": MODEL_ID,
                        "revision": REVISION,
                        "dimensions": DIMENSIONS,
                        "device": DEVICE,
                    },
                )
                return
            self._send(
                503,
                {
                    "status": "loading" if _load_error is None else "error",
                    "ready": False,
                    "model": MODEL_ID,
                    "error": _load_error,
                },
            )
            return
        self._send(404, {"error": {"code": "not_found"}})

    def do_POST(self):
        if self.path != "/v1/embeddings":
            self._send(404, {"error": {"code": "not_found"}})
            return
        if not _authorize(self.headers.get("Authorization")):
            self._send(401, {"error": {"code": "unauthorized"}})
            return
        if not _ready:
            self._send(
                503,
                {
                    "error": {
                        "code": "model_loading",
                        "message": _load_error or "model not ready",
                    }
                },
            )
            return
        length = int(self.headers.get("Content-Length", "0"))
        payload = json.loads(self.rfile.read(length) or b"{}")
        inputs = payload.get("input", [])
        if isinstance(inputs, str):
            inputs = [inputs]
        if not isinstance(inputs, list) or not inputs:
            self._send(400, {"error": {"code": "invalid_input"}})
            return
        try:
            embeddings = embed_batch([str(item) for item in inputs])
        except Exception as error:  # noqa: BLE001
            self._send(500, {"error": {"code": "embedding_failed", "message": str(error)}})
            return
        data = [
            {"object": "embedding", "index": index, "embedding": vector}
            for index, vector in enumerate(embeddings)
        ]
        self._send(
            200,
            {
                "object": "list",
                "data": data,
                "model": payload.get("model", MODEL_ID),
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


def _authorize(header: str | None) -> bool:
    if not API_KEY:
        return True
    if not header or not header.startswith("Bearer "):
        return False
    token = header.removeprefix("Bearer ").strip()
    return token == API_KEY


def main() -> None:
    loader = threading.Thread(target=load_model, name="embedding-model-loader", daemon=True)
    loader.start()
    server = ThreadingHTTPServer((LISTEN_HOST, LISTEN_PORT), Handler)
    server.serve_forever()


if __name__ == "__main__":
    main()
