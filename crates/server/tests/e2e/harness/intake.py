"""Production intake identity extraction (public API only)."""

from __future__ import annotations

import re
from typing import Any

UUID_RE = re.compile(
    r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-5][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$"
)


class ProductionIntakeNotWired(RuntimeError):
    """Upload accepted but production document/version/job identities are absent."""

    code = "production_intake_not_wired"


def _as_uuid(value: Any) -> str | None:
    if isinstance(value, str) and UUID_RE.fullmatch(value):
        return value
    return None


def extract_production_intake(upload_body: dict[str, Any]) -> tuple[str, str, str]:
    """Require production documentId/versionId/jobId from upload (or nested intake).

    objectId alone is insufficient. There is no supported follow-up public API that
    promotes an objectId into a document/version/job in the current `/api/v1` contract.
    """
    nested = upload_body.get("intake")
    nested_obj = nested if isinstance(nested, dict) else {}
    document_id = _as_uuid(upload_body.get("documentId")) or _as_uuid(
        nested_obj.get("documentId")
    )
    version_id = _as_uuid(upload_body.get("versionId")) or _as_uuid(nested_obj.get("versionId"))
    job_id = _as_uuid(upload_body.get("jobId")) or _as_uuid(nested_obj.get("jobId"))
    if document_id and version_id and job_id:
        return document_id, version_id, job_id
    present = sorted(
        key
        for key in ("objectId", "documentId", "versionId", "jobId")
        if upload_body.get(key) is not None or nested_obj.get(key) is not None
    )
    raise ProductionIntakeNotWired(
        "upload response missing production documentId/versionId/jobId "
        f"(present keys: {present or ['<none>']}; objectId-only is not enough; "
        "no supported follow-up public API)"
    )