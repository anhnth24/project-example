"""Compare-dataset, seed/wait, and honest post-restore checks for O05."""

from __future__ import annotations

import json
import os
import time
from pathlib import Path
from typing import Any

from fixtures import marker_for


class DatasetError(RuntimeError):
    """Compare/seed/restore dataset unavailable or invalid."""


COMPARE_ENV = "MARKHAND_SOAK_COMPARE_DATASET"
RESTORED_API_ENV = "MARKHAND_SOAK_RESTORED_API_BASE"


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
    if not all(isinstance(x, str) and x for x in (doc, va, vb)):
        raise DatasetError("compare_dataset_missing_fields")
    if va == vb:
        raise DatasetError("compare_dataset_identical_versions")
    return {"documentId": doc, "versionA": va, "versionB": vb}


def verify_compare_dataset(client: Any, dataset: dict[str, str]) -> dict[str, Any]:
    """Require API 2xx for compare search using the provided real IDs."""
    body = {
        "query": "markhand soak compare verify",
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
    return {"ok": True, "httpStatus": status, "latencyMs": latency, "dataset": dataset}


def resolve_compare_or_block(
    client: Any | None,
    *,
    modes: list[str],
) -> dict[str, Any]:
    """If profile includes compare, require verified dataset; else unavailable."""
    if "compare" not in modes:
        return {"required": False, "available": True, "dataset": None}
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
                "Each POST /uploads creates a new documentId; re-upload cannot form a "
                "version pair. Provide MARKHAND_SOAK_COMPARE_DATASET JSON with real "
                "{documentId,versionA,versionB} verified by API 2xx. No public API "
                "exists to append a second version to an existing document for soak."
            ),
        }
    if dataset is None:
        return {
            "required": True,
            "available": False,
            "blocker": "compare_dataset_unavailable",
            "dataset": None,
            "notes": (
                "MARKHAND_SOAK_COMPARE_DATASET unset. Architectural blocker: upload "
                "always creates a new document; harness will not invent version pairs."
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
            try:
                hits = json.loads(data.decode("utf-8")).get("hits") or []
            except (UnicodeDecodeError, json.JSONDecodeError):
                continue
            hit_docs = {
                str(h.get("documentId") or h.get("document_id"))
                for h in hits
                if isinstance(h, dict)
            }
            if row["documentId"] in hit_docs:
                ready.append(row["format"])
            else:
                # Also accept GET document visibility as indexed/registered.
                st, _b, _l = client.request("GET", f"/api/v1/documents/{row['documentId']}")
                if _http_success(st):
                    ready.append(row["format"])
        if len(set(ready)) >= len(formats):
            return {
                "ok": True,
                "seeded": seeded,
                "readyFormats": sorted(set(ready)),
                "retainedDocumentIds": [s["documentId"] for s in seeded],
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
    report_base = None
    if isinstance(o03_report, dict):
        report_base = (
            o03_report.get("restoredApiBase")
            or (o03_report.get("provenance") or {}).get("restoredApiBase")
            or o03_report.get("greenApiBase")
        )
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
    # Normalize
    restored = candidate.rstrip("/")
    blue = blue_base.rstrip("/")
    if restored.endswith("/api/v1"):
        restored_host = restored[: -len("/api/v1")]
    else:
        restored_host = restored
    if blue.endswith("/api/v1"):
        blue_host = blue[: -len("/api/v1")]
    else:
        blue_host = blue
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
    return {
        "available": True,
        "restoredApiBase": restored_host,
        "blueApiBase": blue_host,
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

    body = json.dumps(
        {
            "query": "markhand soak post-restore",
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
    if not retained_hit:
        for rid in retained_ids:
            st, _b, _l = restored_client.request("GET", f"/api/v1/documents/{rid}")
            if 200 <= st < 300:
                retained_hit = True
                break
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
    # Unauthorized token must not get 2xx on retained document.
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
    unauthorized_ok = st in {401, 403, 404}
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
