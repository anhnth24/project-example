"""Compare-dataset, seed/wait, and honest post-restore checks for O05."""

from __future__ import annotations

import json
import os
import time
import uuid
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any
from urllib.parse import urlparse, urlunparse

from fixtures import marker_for
from redact import url_has_credentials
from workload import search_matches_expected


class DatasetError(RuntimeError):
    """Compare/seed/restore dataset unavailable or invalid."""


COMPARE_ENV = "MARKHAND_SOAK_COMPARE_DATASET"
RESTORED_API_ENV = "MARKHAND_SOAK_RESTORED_API_BASE"


def _normalize_endpoint(value: str) -> str:
    if url_has_credentials(value):
        raise DatasetError("credential_bearing_url_rejected")
    parsed = urlparse(value.strip())
    if parsed.scheme not in {"http", "https"} or not parsed.hostname:
        raise DatasetError("endpoint_invalid")
    scheme = parsed.scheme.lower()
    host = parsed.hostname.lower()
    port = parsed.port
    netloc = host
    if port and not (
        (scheme == "http" and port == 80) or (scheme == "https" and port == 443)
    ):
        netloc = f"{host}:{port}"
    path = parsed.path.rstrip("/")
    if path.endswith("/api/v1"):
        path = path[: -len("/api/v1")]
    return urlunparse((scheme, netloc, path.rstrip("/"), "", "", ""))


def load_compare_dataset(path_or_json: str | None = None) -> dict[str, str] | None:
    """Load explicit compare dataset; never invent IDs."""
    raw = path_or_json or os.environ.get(COMPARE_ENV, "").strip()
    if not raw:
        return None
    if raw.startswith("{"):
        data = json.loads(raw)
    else:
        data = json.loads(Path(raw).read_text(encoding="utf-8"))
    if not isinstance(data, dict):
        raise DatasetError("compare_dataset_not_object")
    doc = data.get("documentId")
    va = data.get("versionA")
    vb = data.get("versionB")
    query = data.get("query")
    marker_a = data.get("markerA")
    marker_b = data.get("markerB")
    effective_a = data.get("effectiveFromA")
    effective_b = data.get("effectiveFromB")
    as_of_a = data.get("asOfA")
    as_of_b = data.get("asOfB")
    required = (
        doc,
        va,
        vb,
        query,
        marker_a,
        marker_b,
        effective_a,
        effective_b,
        as_of_a,
        as_of_b,
    )
    if not all(isinstance(x, str) and x for x in required):
        raise DatasetError("compare_dataset_missing_fields")
    if va == vb:
        raise DatasetError("compare_dataset_identical_versions")
    if marker_a == marker_b:
        raise DatasetError("compare_dataset_identical_markers")
    return {
        "documentId": doc,
        "versionA": va,
        "versionB": vb,
        "query": query,
        "markerA": marker_a,
        "markerB": marker_b,
        "effectiveFromA": effective_a,
        "effectiveFromB": effective_b,
        "asOfA": as_of_a,
        "asOfB": as_of_b,
    }


def _parse_timestamp(value: Any, field: str) -> datetime:
    if not isinstance(value, str) or not value:
        raise DatasetError(f"compare_dataset_{field}_missing")
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as exc:
        raise DatasetError(f"compare_dataset_{field}_invalid") from exc
    if parsed.tzinfo is None:
        raise DatasetError(f"compare_dataset_{field}_timezone_missing")
    return parsed.astimezone(timezone.utc)


def _timestamp_wire(value: datetime) -> str:
    return value.astimezone(timezone.utc).isoformat().replace("+00:00", "Z")


def build_compare_dataset_from_versions(
    *,
    document_id: str,
    version_a: str,
    version_b: str,
    query: str,
    marker_a: str,
    marker_b: str,
    versions: list[dict[str, Any]],
) -> dict[str, str]:
    """Build bounded as-of timestamps from public published-version metadata."""
    by_id = {
        str(row.get("id")): row
        for row in versions
        if isinstance(row, dict) and row.get("id")
    }
    row_a = by_id.get(version_a)
    row_b = by_id.get(version_b)
    if row_a is None or row_b is None:
        raise DatasetError("compare_dataset_versions_missing_from_history")
    if row_a.get("isCurrent") is not False or row_b.get("isCurrent") is not True:
        raise DatasetError("compare_dataset_current_lineage_invalid")
    start_a = _parse_timestamp(row_a.get("effectiveFrom"), "effective_from_a")
    end_a = _parse_timestamp(row_a.get("effectiveTo"), "effective_to_a")
    start_b = _parse_timestamp(row_b.get("effectiveFrom"), "effective_from_b")
    if not start_a < end_a <= start_b:
        raise DatasetError("compare_dataset_effective_window_invalid")
    as_of_a = start_a + (end_a - start_a) / 2
    as_of_b = start_b + timedelta(seconds=1)
    return {
        "documentId": document_id,
        "versionA": version_a,
        "versionB": version_b,
        "query": query,
        "markerA": marker_a,
        "markerB": marker_b,
        "effectiveFromA": _timestamp_wire(start_a),
        "effectiveFromB": _timestamp_wire(start_b),
        "asOfA": _timestamp_wire(as_of_a),
        "asOfB": _timestamp_wire(as_of_b),
    }


def _upload_compare_version(
    client: Any,
    *,
    query: str,
    marker: str,
    suffix: str,
    document_id: str | None,
) -> tuple[str, str]:
    from workload import _multipart_bytes

    body, content_type = _multipart_bytes(
        filename=f"soak-compare-{suffix.lower()}.txt",
        file_bytes=f"{query}\n{marker}\nMarkhand soak version {suffix}\n".encode(),
        collection_id=client.collection_id,
        document_id=document_id,
    )
    headers = client._headers(content_type)
    headers["Idempotency-Key"] = f"o05-compare-{uuid.uuid4().hex}"
    status, data, _latency = client.request(
        "POST", "/api/v1/uploads", body=body, headers=headers
    )
    if not 200 <= status < 300:
        raise DatasetError(f"compare_dataset_upload_{suffix}_failed:http_{status}")
    try:
        payload = json.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise DatasetError(f"compare_dataset_upload_{suffix}_invalid_json") from exc
    got_document = payload.get("documentId")
    got_version = payload.get("versionId")
    if not isinstance(got_document, str) or not isinstance(got_version, str):
        raise DatasetError(f"compare_dataset_upload_{suffix}_missing_ids")
    if document_id is not None and got_document != document_id:
        raise DatasetError("compare_dataset_revision_document_mismatch")
    return got_document, got_version


def _published_versions(client: Any, document_id: str) -> list[dict[str, Any]]:
    status, data, _latency = client.request(
        "GET", f"/api/v1/documents/{document_id}/versions"
    )
    if not 200 <= status < 300:
        raise DatasetError(f"compare_dataset_history_failed:http_{status}")
    try:
        payload = json.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise DatasetError("compare_dataset_history_invalid_json") from exc
    items = payload.get("items") if isinstance(payload, dict) else None
    if not isinstance(items, list) or not all(isinstance(item, dict) for item in items):
        raise DatasetError("compare_dataset_history_items_missing")
    return items


def _current_published_version(versions: list[dict[str, Any]]) -> str:
    current = [
        str(row["id"])
        for row in versions
        if row.get("isCurrent") is True and isinstance(row.get("id"), str)
    ]
    if len(current) != 1:
        raise DatasetError("compare_dataset_current_version_ambiguous")
    return current[0]


def create_compare_dataset(
    client: Any,
    *,
    timeout_seconds: float = 180.0,
    poll_seconds: float = 2.0,
) -> dict[str, str]:
    """Create and verify a real two-version lineage through public HTTP uploads."""
    from workload import wait_until_indexed_visible

    nonce = uuid.uuid4().hex[:12].upper()
    query = f"SOAKCOMPARE{nonce}"
    marker_a = f"SOAKOLDA{nonce}"
    marker_b = f"SOAKNEWB{nonce}"
    document_id, draft_a = _upload_compare_version(
        client,
        query=query,
        marker=marker_a,
        suffix="A",
        document_id=None,
    )
    if not wait_until_indexed_visible(
        client,
        document_id=document_id,
        marker=marker_a,
        timeout_seconds=timeout_seconds,
        poll_seconds=poll_seconds,
    ):
        raise DatasetError("compare_dataset_version_a_index_timeout")
    version_a = _current_published_version(_published_versions(client, document_id))
    revision_document, draft_b = _upload_compare_version(
        client,
        query=query,
        marker=marker_b,
        suffix="B",
        document_id=document_id,
    )
    if revision_document != document_id or draft_a == draft_b:
        raise DatasetError("compare_dataset_revision_lineage_invalid")
    if not wait_until_indexed_visible(
        client,
        document_id=document_id,
        marker=marker_b,
        timeout_seconds=timeout_seconds,
        poll_seconds=poll_seconds,
    ):
        raise DatasetError("compare_dataset_version_b_index_timeout")
    items = _published_versions(client, document_id)
    version_b = _current_published_version(items)
    if version_a == version_b:
        raise DatasetError("compare_dataset_revision_lineage_invalid")
    return build_compare_dataset_from_versions(
        document_id=document_id,
        version_a=version_a,
        version_b=version_b,
        query=query,
        marker_a=marker_a,
        marker_b=marker_b,
        versions=items,
    )


def _version_marker_present(
    payload: dict[str, Any], *, document_id: str, version_id: str, marker: str
) -> bool:
    for hit in payload.get("hits") or []:
        if not isinstance(hit, dict):
            continue
        if str(hit.get("documentId") or hit.get("document_id") or "") != document_id:
            continue
        if str(hit.get("versionId") or hit.get("version_id") or "") != version_id:
            continue
        snippet = hit.get("snippet") or hit.get("quote") or ""
        if isinstance(snippet, str) and marker in snippet:
            return True
    for citation in payload.get("citations") or []:
        if not isinstance(citation, dict):
            continue
        doc = (
            citation.get("logicalDocumentId")
            or citation.get("documentId")
            or citation.get("document_id")
        )
        if str(doc or "") != document_id:
            continue
        if (
            str(citation.get("versionId") or citation.get("version_id") or "")
            != version_id
        ):
            continue
        quote = citation.get("quote")
        if isinstance(quote, str) and marker in quote:
            return True
    return False


def verify_compare_dataset(client: Any, dataset: dict[str, str]) -> dict[str, Any]:
    """Require API 2xx plus expected compare hit/citation using provided real IDs."""
    body = {
        "query": dataset["query"],
        "mode": "compare",
        "limit": 5,
        "collectionIds": [client.collection_id],
        "documentId": dataset["documentId"],
        "versionA": dataset["versionA"],
        "versionB": dataset["versionB"],
    }
    status, data, latency = client.request(
        "POST", "/api/v1/search", body=json.dumps(body).encode("utf-8")
    )
    if not (200 <= status < 300):
        raise DatasetError(f"compare_dataset_api_rejected:http_{status}")
    try:
        payload = json.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise DatasetError("compare_dataset_invalid_json") from exc
    if not isinstance(payload, dict):
        raise DatasetError("compare_dataset_not_object_response")
    for version_key, marker_key in (("versionA", "markerA"), ("versionB", "markerB")):
        if not _version_marker_present(
            payload,
            document_id=dataset["documentId"],
            version_id=dataset[version_key],
            marker=dataset[marker_key],
        ):
            raise DatasetError(
                f"compare_dataset_expected_version_missing:{version_key}"
            )
    as_of_checks: dict[str, dict[str, Any]] = {}
    for suffix in ("A", "B"):
        as_of_body = {
            "query": dataset[f"marker{suffix}"],
            "mode": "as_of",
            "asOf": dataset[f"asOf{suffix}"],
            "limit": 5,
            "collectionIds": [client.collection_id],
            "documentId": dataset["documentId"],
        }
        as_status, as_data, as_latency = client.request(
            "POST",
            "/api/v1/search",
            body=json.dumps(as_of_body).encode("utf-8"),
        )
        if not 200 <= as_status < 300:
            raise DatasetError(
                f"compare_dataset_as_of_{suffix.lower()}_rejected:http_{as_status}"
            )
        try:
            as_payload = json.loads(as_data.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            raise DatasetError(
                f"compare_dataset_as_of_{suffix.lower()}_invalid_json"
            ) from exc
        if not isinstance(as_payload, dict) or not _version_marker_present(
            as_payload,
            document_id=dataset["documentId"],
            version_id=dataset[f"version{suffix}"],
            marker=dataset[f"marker{suffix}"],
        ):
            raise DatasetError(
                f"compare_dataset_as_of_{suffix.lower()}_expected_version_missing"
            )
        as_of_checks[suffix] = {
            "httpStatus": as_status,
            "latencyMs": as_latency,
        }
    return {
        "ok": True,
        "httpStatus": status,
        "latencyMs": latency,
        "asOf": as_of_checks,
        "dataset": dataset,
    }


def resolve_compare_or_block(
    client: Any | None,
    *,
    modes: list[str],
    create_if_missing: bool = False,
    timeout_seconds: float = 180.0,
) -> dict[str, Any]:
    """If profile includes compare, require verified dataset; else unavailable."""
    if "compare" not in modes:
        return {"required": False, "available": True, "dataset": None}
    source = "configured_dataset"
    try:
        dataset = load_compare_dataset()
    except (OSError, json.JSONDecodeError, DatasetError) as exc:
        return {
            "required": True,
            "available": False,
            "blocker": "compare_dataset_unavailable",
            "error": str(exc),
            "dataset": None,
            "notes": (
                "Configured compare dataset is malformed; refusing to replace explicit "
                "operator input with an automatically generated lineage."
            ),
        }
    if dataset is None:
        if create_if_missing and client is not None:
            try:
                dataset = create_compare_dataset(
                    client, timeout_seconds=timeout_seconds
                )
                source = "public_revision_upload"
            except DatasetError as exc:
                return {
                    "required": True,
                    "available": False,
                    "blocker": "compare_dataset_unavailable",
                    "dataset": None,
                    "error": str(exc),
                    "source": "public_revision_upload",
                }
        else:
            return {
                "required": True,
                "available": False,
                "blocker": "compare_dataset_unavailable",
                "dataset": None,
                "notes": (
                    "MARKHAND_SOAK_COMPARE_DATASET unset and public revision "
                    "preflight was not eligible to run."
                ),
            }
    if client is None:
        return {
            "required": True,
            "available": False,
            "blocker": "compare_dataset_unavailable",
            "dataset": dataset,
            "error": "no_client_to_verify",
        }
    try:
        verified = verify_compare_dataset(client, dataset)
    except DatasetError as exc:
        return {
            "required": True,
            "available": False,
            "blocker": "compare_dataset_unavailable",
            "dataset": dataset,
            "error": str(exc),
        }
    return {
        "required": True,
        "available": True,
        "dataset": dataset,
        "verified": verified,
        "source": source,
    }


def seed_and_wait_indexed(
    client: Any,
    *,
    formats: list[str],
    fixture_path_fn,
    timeout_seconds: float = 180.0,
    poll_seconds: float = 2.0,
) -> dict[str, Any]:
    """Upload one fixture per format and wait until search returns marker hits.

    Ensures profile actors are executable from t=0 of the timed schedule.
    """
    from workload import _http_success, _multipart

    seeded: list[dict[str, Any]] = []
    for fmt in formats:
        path = fixture_path_fn(fmt)
        body, content_type = _multipart(path, client.collection_id)
        status, data, _lat = client.request(
            "POST",
            "/api/v1/uploads",
            body=body,
            headers=client._headers(content_type),
        )
        if not _http_success(status):
            raise DatasetError(f"seed_upload_failed:{fmt}:http_{status}")
        payload = json.loads(data.decode("utf-8"))
        doc_id = payload.get("documentId")
        ver_id = payload.get("versionId")
        if not isinstance(doc_id, str) or not isinstance(ver_id, str):
            raise DatasetError(f"seed_upload_missing_ids:{fmt}")
        seeded.append(
            {
                "format": fmt,
                "documentId": doc_id,
                "versionId": ver_id,
                "marker": marker_for(fmt),
            }
        )

    deadline = time.monotonic() + timeout_seconds
    ready: list[str] = []
    while time.monotonic() < deadline:
        ready = []
        for row in seeded:
            body = json.dumps(
                {
                    "query": row["marker"],
                    "mode": "current",
                    "limit": 5,
                    "collectionIds": [client.collection_id],
                }
            ).encode("utf-8")
            status, data, _lat = client.request("POST", "/api/v1/search", body=body)
            if not _http_success(status):
                continue
            if search_matches_expected(
                data,
                expected_doc=row["documentId"],
                expected_version=row["versionId"],
                expected_marker=row["marker"],
                require_citation=True,
            ):
                ready.append(row["format"])
        if len(set(ready)) >= len(formats):
            return {
                "ok": True,
                "seeded": seeded,
                "readyFormats": sorted(set(ready)),
                "retainedDocumentIds": [s["documentId"] for s in seeded],
                "retainedMarkerHashes": {
                    s["documentId"]: __import__("hashlib")
                    .sha256(s["marker"].encode("utf-8"))
                    .hexdigest()
                    for s in seeded
                },
            }
        time.sleep(poll_seconds)
    raise DatasetError(
        "seed_index_timeout:"
        + json.dumps({"ready": sorted(set(ready)), "expected": sorted(formats)})
    )


def resolve_restored_api_base(
    *,
    blue_base: str,
    o03_report: dict[str, Any] | None,
) -> dict[str, Any]:
    """Locate a true restored/green API endpoint. Blue==restored is non-pass."""
    env_base = os.environ.get(RESTORED_API_ENV, "").strip()
    env_green_identity = os.environ.get(
        "MARKHAND_SOAK_RESTORED_DEPLOYMENT_IDENTITY", ""
    ).strip()
    env_blue_identity = os.environ.get(
        "MARKHAND_SOAK_BLUE_DEPLOYMENT_IDENTITY", ""
    ).strip()
    env_green_storage = os.environ.get(
        "MARKHAND_SOAK_RESTORED_STORAGE_SIGNATURE", ""
    ).strip()
    env_blue_storage = os.environ.get(
        "MARKHAND_SOAK_BLUE_STORAGE_SIGNATURE", ""
    ).strip()
    report_base = None
    green_identity = env_green_identity or None
    blue_identity = env_blue_identity or None
    green_storage = env_green_storage or None
    blue_storage = env_blue_storage or None
    if isinstance(o03_report, dict):
        report_base = (
            o03_report.get("restoredApiBase")
            or (o03_report.get("provenance") or {}).get("restoredApiBase")
            or o03_report.get("greenApiBase")
        )
        green_identity = (
            green_identity
            or o03_report.get("restoredDeploymentIdentity")
            or o03_report.get("greenDeploymentIdentity")
        )
        blue_identity = blue_identity or o03_report.get("blueDeploymentIdentity")
        green_storage = (
            green_storage
            or o03_report.get("restoredStorageSignature")
            or o03_report.get("greenStorageSignature")
        )
        blue_storage = blue_storage or o03_report.get("blueStorageSignature")
    candidate = env_base or (report_base if isinstance(report_base, str) else None)
    if not candidate:
        return {
            "available": False,
            "blocker": "restored_api_base_missing",
            "notes": (
                "O03 restores an isolated green stack with promote/cutover disabled; "
                "the blue MARKHAND_SOAK_API_BASE is not post-restore proof. Set "
                "MARKHAND_SOAK_RESTORED_API_BASE or have O03 evidence expose "
                "restoredApiBase for a reachable green endpoint."
            ),
            "restoredApiBase": None,
            "blueApiBase": blue_base,
        }
    try:
        restored_host = _normalize_endpoint(candidate)
        blue_host = _normalize_endpoint(blue_base)
    except DatasetError as exc:
        return {
            "available": False,
            "blocker": str(exc),
            "restoredApiBase": None,
            "blueApiBase": blue_base,
        }
    if restored_host == blue_host:
        return {
            "available": False,
            "blocker": "restored_api_same_as_blue",
            "notes": (
                "Restored API base equals blue soak API; promote/cutover is disabled "
                "so this cannot be post-restore evidence."
            ),
            "restoredApiBase": restored_host,
            "blueApiBase": blue_host,
        }
    if not all(
        isinstance(x, str) and x
        for x in (green_identity, blue_identity, green_storage, blue_storage)
    ):
        return {
            "available": False,
            "blocker": "restored_green_identity_missing",
            "notes": (
                "Post-restore proof needs distinct green deployment identity and "
                "storage signature, not only a different URL alias."
            ),
            "restoredApiBase": restored_host,
            "blueApiBase": blue_host,
        }
    green_db = (
        o03_report.get("greenDbIdentity") if isinstance(o03_report, dict) else None
    )
    blue_db = o03_report.get("blueDbIdentity") if isinstance(o03_report, dict) else None
    green_bucket = (
        o03_report.get("greenMinioBucket") if isinstance(o03_report, dict) else None
    )
    blue_bucket = (
        o03_report.get("blueMinioBucket") if isinstance(o03_report, dict) else None
    )
    green_qdrant = (
        o03_report.get("greenQdrantCollection")
        if isinstance(o03_report, dict)
        else None
    )
    blue_qdrant = (
        o03_report.get("blueQdrantCollection") if isinstance(o03_report, dict) else None
    )
    if green_identity == blue_identity or green_storage == blue_storage:
        return {
            "available": False,
            "blocker": "restored_green_identity_not_distinct",
            "notes": "Green endpoint/storage identity equals blue; aliases cannot prove restore.",
            "restoredApiBase": restored_host,
            "blueApiBase": blue_host,
        }
    runtime_pairs = [
        ("db", green_db, blue_db),
        ("minioBucket", green_bucket, blue_bucket),
        ("qdrantCollection", green_qdrant, blue_qdrant),
    ]
    for name, green, blue_value in runtime_pairs:
        if isinstance(green, str) or isinstance(blue_value, str):
            if not (
                isinstance(green, str)
                and green
                and isinstance(blue_value, str)
                and blue_value
            ):
                return {
                    "available": False,
                    "blocker": f"restored_{name}_identity_missing",
                }
            if green == blue_value:
                return {
                    "available": False,
                    "blocker": f"restored_{name}_identity_not_distinct",
                }
    return {
        "available": True,
        "restoredApiBase": restored_host,
        "blueApiBase": blue_host,
        "restoredDeploymentIdentity": green_identity,
        "blueDeploymentIdentity": blue_identity,
        "restoredStorageSignature": green_storage,
        "blueStorageSignature": blue_storage,
        "greenDbIdentity": green_db,
        "blueDbIdentity": blue_db,
        "greenMinioBucket": green_bucket,
        "blueMinioBucket": blue_bucket,
        "greenQdrantCollection": green_qdrant,
        "blueQdrantCollection": blue_qdrant,
        "source": "env" if env_base else "o03_report",
    }


def post_restore_retrieval_check(
    restored_client: Any,
    *,
    retained_ids: list[str],
    deleted_ids: list[str],
    unauthorized_client: Any | None,
    same_run_restore: bool,
    restored_endpoint_ok: bool,
    retained_markers: dict[str, str] | None = None,
    deleted_markers: dict[str, str] | None = None,
) -> dict[str, Any]:
    """Retained authorized hit + deleted suppression + unauthorized denial on green."""
    if not same_run_restore or not restored_endpoint_ok:
        return {
            "passed": None,
            "gate": "unknown",
            "reason": "no_reachable_restored_endpoint",
        }
    if not retained_ids:
        return {"passed": False, "gate": "fail", "reason": "no_retained_ids"}
    if not deleted_ids:
        return {"passed": False, "gate": "fail", "reason": "no_deleted_ids"}
    overlap = sorted(set(retained_ids).intersection(deleted_ids))
    if overlap:
        return {
            "passed": False,
            "gate": "fail",
            "reason": "retained_deleted_overlap",
            "overlapCount": len(overlap),
        }

    marker = None
    for rid in retained_ids:
        if retained_markers and retained_markers.get(rid):
            marker = retained_markers[rid]
            break
    if not marker:
        return {"passed": False, "gate": "fail", "reason": "retained_marker_missing"}

    for rid in retained_ids:
        st_doc, _body_doc, _lat_doc = restored_client.request(
            "GET", f"/api/v1/documents/{rid}"
        )
        if not (200 <= st_doc < 300):
            return {
                "passed": False,
                "gate": "fail",
                "reason": "retained_terminal_missing",
                "documentId": rid,
                "status": st_doc,
            }
    for did in deleted_ids:
        st_doc, _body_doc, _lat_doc = restored_client.request(
            "GET", f"/api/v1/documents/{did}"
        )
        if 200 <= st_doc < 300:
            return {
                "passed": False,
                "gate": "fail",
                "reason": "deleted_terminal_visible",
                "documentId": did,
                "status": st_doc,
            }

    body = json.dumps(
        {
            "query": marker,
            "mode": "current",
            "limit": 20,
            "collectionIds": [restored_client.collection_id],
        }
    ).encode("utf-8")
    status, data, _lat = restored_client.request("POST", "/api/v1/search", body=body)
    if not (200 <= status < 300):
        return {"passed": False, "gate": "fail", "reason": f"search_status_{status}"}
    try:
        payload = json.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError):
        return {"passed": False, "gate": "fail", "reason": "invalid_json"}
    hits = payload.get("hits") or []
    hit_docs = set()
    for hit in hits:
        if isinstance(hit, dict):
            for key in ("documentId", "document_id"):
                if hit.get(key):
                    hit_docs.add(str(hit.get(key)))
    leaked = [d for d in deleted_ids if d in hit_docs]
    retained_hit = any(r in hit_docs for r in retained_ids)
    marker_ok = search_matches_expected(
        data,
        expected_doc=retained_ids[0],
        expected_marker=marker,
        require_citation=True,
    )
    if not retained_hit or not marker_ok:
        retained_hit = False
    if not retained_hit:
        return {
            "passed": False,
            "gate": "fail",
            "reason": "retained_hit_absent",
            "leakedDeletedIds": len(leaked),
        }
    if leaked:
        return {
            "passed": False,
            "gate": "fail",
            "reason": "deleted_still_visible",
            "leakedDeletedIds": len(leaked),
            "retainedVisible": True,
        }

    unauthorized_ok = False
    if unauthorized_client is None:
        return {
            "passed": False,
            "gate": "fail",
            "reason": "unauthorized_client_missing",
            "retainedVisible": True,
            "leakedDeletedIds": 0,
        }
    for did, deleted_marker in (deleted_markers or {}).items():
        body_deleted = json.dumps(
            {
                "query": deleted_marker,
                "mode": "current",
                "limit": 20,
                "collectionIds": [restored_client.collection_id],
            }
        ).encode("utf-8")
        st_deleted, data_deleted, _lat_deleted = restored_client.request(
            "POST", "/api/v1/search", body=body_deleted
        )
        if 200 <= st_deleted < 300:
            try:
                deleted_payload = json.loads(data_deleted.decode("utf-8"))
            except (UnicodeDecodeError, json.JSONDecodeError):
                deleted_payload = {}
            if did in {
                str(hit.get("documentId") or hit.get("document_id"))
                for hit in deleted_payload.get("hits", [])
                if isinstance(hit, dict)
            }:
                return {
                    "passed": False,
                    "gate": "fail",
                    "reason": "deleted_marker_visible",
                    "documentId": did,
                }
    # An unauthenticated or authenticated-but-unscoped context must not receive
    # 2xx or text-bearing results from the restored deployment.
    body_unauth = json.dumps(
        {
            "query": marker,
            "mode": "current",
            "limit": 5,
            "collectionIds": [unauthorized_client.collection_id],
        }
    ).encode("utf-8")
    st_search, data_search, _l_search = unauthorized_client.request(
        "POST", "/api/v1/search", body=body_unauth
    )
    if 200 <= st_search < 300:
        return {
            "passed": False,
            "gate": "fail",
            "reason": "unauthorized_search_2xx",
            "unauthorizedSearchStatus": st_search,
        }
    if st_search not in {401, 403}:
        return {
            "passed": False,
            "gate": "fail",
            "reason": f"unauthorized_search_unexpected_status_{st_search}",
        }
    st, _b, _l = unauthorized_client.request(
        "GET", f"/api/v1/documents/{retained_ids[0]}"
    )
    if 200 <= st < 300:
        return {
            "passed": False,
            "gate": "fail",
            "reason": "unauthorized_access_2xx",
            "unauthorizedStatus": st,
        }
    unauthorized_ok = st in {401, 403}
    if not unauthorized_ok:
        return {
            "passed": False,
            "gate": "fail",
            "reason": f"unauthorized_unexpected_status_{st}",
        }
    return {
        "passed": True,
        "gate": "pass",
        "retainedVisible": True,
        "leakedDeletedIds": 0,
        "unauthorizedDenied": True,
        "unauthorizedStatus": st,
        "sameRunRestore": True,
    }
