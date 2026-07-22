#!/usr/bin/env python3
"""Qdrant v1.18.2 collection/snapshot response parsing (fail-closed)."""

from __future__ import annotations

from typing import Any


class QdrantApiError(ValueError):
    """Fail-closed Qdrant API parse error."""


def parse_collection_info(payload: dict[str, Any]) -> dict[str, Any]:
    """Parse GET /collections/{name} for Qdrant 1.18.2.

    Real shape uses top-level ``status`` string and top-level
    ``points_count`` / ``indexed_vectors_count`` / ``config`` under ``result``.
    Do not treat status as a dict or call status.get.
    """
    if not isinstance(payload, dict):
        raise QdrantApiError("collection info root must be object")
    result = payload.get("result")
    if not isinstance(result, dict):
        raise QdrantApiError("collection info missing result object")
    status = result.get("status")
    if not isinstance(status, str) or not status:
        raise QdrantApiError(
            f"collection status must be non-empty string, got {type(status).__name__}"
        )
    points = result.get("points_count")
    if not isinstance(points, int) or isinstance(points, bool) or points < 0:
        raise QdrantApiError("points_count must be non-negative integer")
    indexed = result.get("indexed_vectors_count")
    if not isinstance(indexed, int) or isinstance(indexed, bool) or indexed < 0:
        raise QdrantApiError("indexed_vectors_count must be non-negative integer")
    config = result.get("config")
    if not isinstance(config, dict):
        raise QdrantApiError("config must be object")
    params = config.get("params") if isinstance(config.get("params"), dict) else config
    vectors = params.get("vectors") if isinstance(params, dict) else None
    if not isinstance(vectors, dict):
        raise QdrantApiError("collection config.vectors missing")
    return {
        "status": status,
        "pointsCount": points,
        "indexedVectorsCount": indexed,
        "config": config,
        "vectors": vectors,
    }


def parse_snapshot_create(payload: dict[str, Any]) -> str:
    if not isinstance(payload, dict):
        raise QdrantApiError("snapshot create root must be object")
    result = payload.get("result")
    if isinstance(result, dict):
        name = result.get("name")
    else:
        name = result
    if not isinstance(name, str) or not name:
        raise QdrantApiError("snapshot create missing name")
    return name


def parse_recover_response(payload: dict[str, Any]) -> None:
    if not isinstance(payload, dict):
        raise QdrantApiError("recover response must be object")
    # 1.18.2 upload/recover returns {"result": true, "status": "ok"} or similar.
    result = payload.get("result")
    status = payload.get("status")
    if result is True:
        return
    if status == "ok" and result in {True, "ok", None}:
        return
    if isinstance(result, dict) and result.get("status") in {"ok", "completed"}:
        return
    raise QdrantApiError(f"recover response not successful: status={status!r}")


def assert_collection_matches(
    info: dict[str, Any],
    *,
    expected_points: int,
    expected_vectors: dict[str, Any],
    require_green: bool = True,
) -> None:
    if require_green and info["status"] not in {"green", "yellow"}:
        raise QdrantApiError(f"collection status not ready: {info['status']}")
    if info["pointsCount"] != expected_points:
        raise QdrantApiError(
            f"points_count mismatch: got {info['pointsCount']} want {expected_points}"
        )
    # Vector size/distance identity check when declared.
    exp_size = expected_vectors.get("size")
    exp_dist = expected_vectors.get("distance")
    got = info["vectors"]
    # Support named/unnamed vectors.
    if "size" in got:
        size = got.get("size")
        distance = got.get("distance")
    else:
        # pick first named vector
        first = next(iter(got.values()), None)
        if not isinstance(first, dict):
            raise QdrantApiError("unable to read vectors config")
        size = first.get("size")
        distance = first.get("distance")
    if exp_size is not None and size != exp_size:
        raise QdrantApiError(f"vector size mismatch: {size} != {exp_size}")
    if exp_dist is not None and distance != exp_dist:
        raise QdrantApiError(f"vector distance mismatch: {distance} != {exp_dist}")
