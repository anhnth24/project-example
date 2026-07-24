#!/usr/bin/env python3
"""Idempotently create the Qdrant collection required by API readiness."""

from __future__ import annotations

import json
import os
import re
import time
import urllib.error
import urllib.request


def required_config() -> tuple[str, int, str]:
    signature = os.environ.get("MARKHAND_INDEX_SIGNATURE", "")
    if not re.fullmatch(r"[a-f0-9]{64}", signature):
        raise SystemExit("MARKHAND_INDEX_SIGNATURE must be 64 lowercase hex characters")
    try:
        dimensions = int(os.environ.get("MARKHAND_EMBEDDING_DIMENSIONS", "0"))
    except ValueError as error:
        raise SystemExit("MARKHAND_EMBEDDING_DIMENSIONS must be an integer") from error
    if dimensions <= 0:
        raise SystemExit("MARKHAND_EMBEDDING_DIMENSIONS must be positive")
    base_url = os.environ.get("MARKHAND_QDRANT_URL", "http://qdrant:6333").rstrip("/")
    return signature, dimensions, base_url


def request_json(url: str, *, method: str = "GET", body: dict | None = None) -> tuple[int, dict]:
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(
        url,
        data=data,
        method=method,
        headers={"Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(request, timeout=3) as response:
            raw = response.read().decode(errors="replace")
            try:
                payload = json.loads(raw)
            except json.JSONDecodeError:
                payload = {}
            return response.status, payload
    except urllib.error.HTTPError as error:
        raw = error.read().decode(errors="replace")
        try:
            payload = json.loads(raw)
        except json.JSONDecodeError:
            payload = {"error": raw}
        return error.code, payload


def vector_config(payload: dict) -> tuple[int, str] | None:
    vectors = (
        payload.get("result", {})
        .get("config", {})
        .get("params", {})
        .get("vectors")
    )
    if not isinstance(vectors, dict):
        return None
    size = vectors.get("size")
    distance = vectors.get("distance")
    if not isinstance(size, int) or not isinstance(distance, str):
        return None
    return size, distance.lower()


def main() -> int:
    signature, dimensions, base_url = required_config()
    name = f"markhand_chunks_{signature}"
    collection_url = f"{base_url}/collections/{name}"

    deadline = time.monotonic() + 90
    while True:
        try:
            status, _ = request_json(f"{base_url}/healthz")
            if 200 <= status < 300:
                break
        except (OSError, urllib.error.URLError):
            pass
        if time.monotonic() >= deadline:
            raise SystemExit("qdrant did not become healthy")
        time.sleep(1)

    status, payload = request_json(collection_url)
    if status == 404:
        status, payload = request_json(
            collection_url,
            method="PUT",
            body={"vectors": {"size": dimensions, "distance": "Cosine"}},
        )
        if not 200 <= status < 300:
            raise SystemExit(f"failed to create Qdrant collection (status={status})")
        status, payload = request_json(collection_url)

    if not 200 <= status < 300:
        raise SystemExit(f"failed to inspect Qdrant collection (status={status})")
    actual = vector_config(payload)
    expected = (dimensions, "cosine")
    if actual != expected:
        raise SystemExit(f"Qdrant collection mismatch: got={actual!r} expected={expected!r}")

    print(f"qdrant collection ready: {name} dimensions={dimensions} distance=Cosine")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
