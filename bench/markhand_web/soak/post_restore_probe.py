#!/usr/bin/env python3
"""Run O05's retained/deleted/authz proof while O03 green is still alive."""

from __future__ import annotations

import argparse
import json
import os
import tempfile
from pathlib import Path
from typing import Any

import dataset
import workload


class ProbeError(RuntimeError):
    """The bounded external green probe could not produce valid evidence."""


def _required_string(value: Any, name: str) -> str:
    if not isinstance(value, str) or not value:
        raise ProbeError(f"{name}_missing")
    return value


def load_request(path: Path) -> dict[str, Any]:
    if path.is_symlink() or not path.is_file():
        raise ProbeError("probe_request_not_regular_file")
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ProbeError("probe_request_invalid") from exc
    if not isinstance(payload, dict):
        raise ProbeError("probe_request_not_object")
    collection_id = _required_string(payload.get("collectionId"), "collection_id")
    retained = payload.get("retainedIds")
    deleted = payload.get("deletedIds")
    retained_markers = payload.get("retainedMarkers")
    deleted_markers = payload.get("deletedMarkers")
    if not isinstance(retained, list) or not all(
        isinstance(value, str) and value for value in retained
    ):
        raise ProbeError("retained_ids_invalid")
    if not isinstance(deleted, list) or not all(
        isinstance(value, str) and value for value in deleted
    ):
        raise ProbeError("deleted_ids_invalid")
    if not isinstance(retained_markers, dict) or not all(
        isinstance(key, str) and key and isinstance(value, str) and value
        for key, value in retained_markers.items()
    ):
        raise ProbeError("retained_markers_invalid")
    if not isinstance(deleted_markers, dict) or not all(
        isinstance(key, str) and key and isinstance(value, str) and value
        for key, value in deleted_markers.items()
    ):
        raise ProbeError("deleted_markers_invalid")
    return {
        "collectionId": collection_id,
        "retainedIds": retained,
        "deletedIds": deleted,
        "retainedMarkers": retained_markers,
        "deletedMarkers": deleted_markers,
    }


def run_probe(api_base: str, request: dict[str, Any]) -> dict[str, Any]:
    restored_base = dataset._normalize_endpoint(api_base)
    blue_base = dataset._normalize_endpoint(
        _required_string(os.environ.get("MARKHAND_SOAK_API_BASE"), "blue_api_base")
    )
    if restored_base == blue_base:
        raise ProbeError("restored_api_same_as_blue")
    email = os.environ.get("MARKHAND_SOAK_EMAIL", "admin@poc.example")
    password = _required_string(
        os.environ.get("MARKHAND_SOAK_PASSWORD"), "soak_password"
    )
    token = workload.login(restored_base, email, password)
    collection_id = request["collectionId"]
    restored = workload.ApiClient(
        restored_base,
        token=token,
        collection_id=collection_id,
        timeout_seconds=float(os.environ.get("MARKHAND_SOAK_TIMEOUT_SECONDS", "30")),
    )
    unauthorized = workload.ApiClient(
        restored_base,
        token=None,
        collection_id=collection_id,
        timeout_seconds=10.0,
    )
    post_restore = dataset.post_restore_retrieval_check(
        restored,
        retained_ids=request["retainedIds"],
        deleted_ids=request["deletedIds"],
        unauthorized_client=unauthorized,
        same_run_restore=True,
        restored_endpoint_ok=True,
        retained_markers=request["retainedMarkers"],
        deleted_markers=request["deletedMarkers"],
    )
    restored_api = {
        "available": True,
        "restoredApiBase": restored_base,
        "blueApiBase": blue_base,
        "restoredDeploymentIdentity": _required_string(
            os.environ.get("MARKHAND_O03_GREEN_DEPLOYMENT_ID"),
            "green_deployment_identity",
        ),
        "blueDeploymentIdentity": _required_string(
            os.environ.get("MARKHAND_O03_BLUE_DEPLOYMENT_ID"),
            "blue_deployment_identity",
        ),
        "restoredStorageSignature": _required_string(
            os.environ.get("MARKHAND_O03_GREEN_STORAGE_SIGNATURE"),
            "green_storage_signature",
        ),
        "blueStorageSignature": _required_string(
            os.environ.get("MARKHAND_O03_BLUE_STORAGE_SIGNATURE"),
            "blue_storage_signature",
        ),
        "source": "o03_live_external_probe",
    }
    if (
        restored_api["restoredDeploymentIdentity"]
        == restored_api["blueDeploymentIdentity"]
        or restored_api["restoredStorageSignature"]
        == restored_api["blueStorageSignature"]
    ):
        raise ProbeError("restored_identity_not_distinct")
    return {
        "schemaVersion": 1,
        "passed": post_restore.get("passed") is True,
        "postRestore": post_restore,
        "restoredApi": restored_api,
    }


def write_output(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        mode="w",
        encoding="utf-8",
        dir=path.parent,
        prefix=f".{path.name}.",
        delete=False,
    ) as handle:
        json.dump(payload, handle, indent=2, sort_keys=True)
        handle.write("\n")
        temporary = Path(handle.name)
    os.chmod(temporary, 0o600)
    temporary.replace(path)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--api-base", required=True)
    parser.add_argument("--request", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    try:
        payload = run_probe(args.api_base, load_request(args.request))
    except (ProbeError, dataset.DatasetError, RuntimeError, OSError) as exc:
        payload = {
            "schemaVersion": 1,
            "passed": False,
            "error": type(exc).__name__,
        }
        write_output(args.output, payload)
        return 1
    write_output(args.output, payload)
    return 0 if payload["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
