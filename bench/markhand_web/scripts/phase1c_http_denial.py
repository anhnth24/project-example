#!/usr/bin/env python3
"""Black-box HTTP/SSE denial driver for Phase 1C deployed leakage proof."""

from __future__ import annotations

import hashlib
import json
import re
import secrets
import uuid
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Protocol

ROOT = Path(__file__).resolve().parents[3]
DEFAULT_MANIFEST = ROOT / "crates/server/tests/fixtures/multi-org-denial.manifest.json"
DEFAULT_GUARD_INVENTORY = ROOT / "crates/server/openapi/guard-inventory.json"
API_PREFIX = "/api/v1"
MULTIPART_BOUNDARY = "----markhandPhase1cDenialBoundary"
CANONICAL_EXECUTABLE_HTTP_SSE_COUNT = 60

HttpRequestFn = Callable[..., Any]

UUID_RE = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$",
    re.IGNORECASE,
)

DISPOSABLE_COLLECTION_DELETE_OPS = frozenset({"deleteCollection"})
DISPOSABLE_COLLECTION_UPDATE_OPS = frozenset({"updateCollection"})
DISPOSABLE_DOCUMENT_OPS = frozenset({"deleteDocument"})
DISPOSABLE_CHAT_OPS = frozenset({"deleteChatSession"})
DISPOSABLE_INVITE_OPS = frozenset({"revokeMemberInvite"})
DISPOSABLE_MEMBER_PATCH_OPS = frozenset({"patchMember"})
DISPOSABLE_MEMBER_DELETE_OPS = frozenset({"deleteMember"})
DISPOSABLE_CONFLICT_OPS = frozenset({"triageConflict"})

DUPLICATE_COLLECTION_NAME = "Shared Contract Collection"
CURRENT_ORG_READ_OPS = frozenset(
    {
        "authMe",
        "getGraph",
        "getUsage",
        "listCollections",
        "listDocuments",
        "listProjects",
        "listChatSessions",
        "listOrgs",
        "listMembers",
        "listMemberInvites",
        "listConflicts",
    }
)
APPROVE_INTAKE_OPS = frozenset({"approveIntake"})
SSE_ENVELOPE_REQUIRED_KEYS = frozenset({"version", "sequence", "event", "requestId", "data"})
SSE_MAX_BYTES = 65536
SSE_MAX_SECONDS = 30.0

SECONDARY_ROW_IDS: frozenset[str] = frozenset(
    {
        "denial-resolveCitation-citation",
        "denial-previewDocument-citation",
        "denial-task13-indexed-fts-ask",
        "denial-task13-duplicate-names",
        "denial-task13-stale-tokens",
        "denial-task13-preview-download-sse",
        "denial-task13-in-flight-ask-revoke",
    }
)

SSE_TERMINAL_EVENT = "stream.closed"

# Owner-control success JSON keys aligned with OpenAPI + canonical Rust integration tests.
HTTP_OWNER_SUCCESS_SCHEMA: dict[str, frozenset[str]] = {
    "authMe": frozenset({"userId", "sessionId", "orgId"}),
    "diffDocumentVersions": frozenset({"documentId", "left", "right", "note", "requestId"}),
    "getChatSession": frozenset({"id", "title", "turns"}),
    "getCollection": frozenset({"id", "name"}),
    "getConflict": frozenset({"id", "status"}),
    "getConflictEvidence": frozenset({"items"}),
    "getDocument": frozenset({"id", "title"}),
    "getDocumentVersion": frozenset(
        {"id", "documentId", "versionNumber", "isCurrent", "sourceContentSha256"}
    ),
    "getGraph": frozenset({"nodes"}),
    "getJob": frozenset({"id", "status"}),
    "getOrg": frozenset({"id", "name"}),
    "getProject": frozenset({"id", "name"}),
    "getUsage": frozenset({"items"}),
    "listAudit": frozenset({"items", "page"}),
    "listChatSessions": frozenset({"items"}),
    "listCollections": frozenset({"items"}),
    "listConflicts": frozenset({"items", "requestId"}),
    "listDocumentVersions": frozenset({"items"}),
    "listDocuments": frozenset({"items"}),
    "listMemberInvites": frozenset({"items"}),
    "listMembers": frozenset({"items"}),
    "listOrgs": frozenset({"items"}),
    "listProjects": frozenset({"items"}),
    "previewDocument": frozenset({"documentId", "versionId"}),
    "resolveCitation": frozenset({"citation", "requestId"}),
}

OPERATION_FOREIGN_DENIAL: dict[str, frozenset[int]] = {
    "switchOrg": frozenset({403}),
    "redeemDownload": frozenset({400}),
    "acceptMemberInvite": frozenset({404}),
    "approveIntake": frozenset({403, 404}),
}

DEFAULT_FOREIGN_DENIAL = frozenset({403, 404})


class HttpResponseLike(Protocol):
    status: int
    body: str
    headers: dict[str, str]


@dataclass(frozen=True)
class ManifestRow:
    row_id: str
    operation_id: str
    layer: str
    test_name: str
    binary: str
    evidence_role: str


@dataclass(frozen=True)
class GuardRoute:
    operation_id: str
    method: str
    path_template: str
    authz_kind: str


@dataclass
class DenialMappingEntry:
    row_id: str
    operation_id: str
    layer: str
    method: str
    path_template: str
    authz_kind: str
    test_name: str
    binary: str
    evidence_role: str


@dataclass
class OwnerControlSpec:
    method: str
    path: str
    body: dict[str, Any] | None
    expected_statuses: frozenset[int]
    content_type: str = "application/json"
    multipart_body: bytes | None = None
    accept: str | None = None
    success_schema_keys: frozenset[str] | None = None
    token: str | None = None


@dataclass
class DenialRequestSpec:
    row_id: str
    operation_id: str
    scenario: str
    method: str
    path: str
    token: str | None
    body: dict[str, Any] | None
    expected_statuses: frozenset[int]
    content_type: str = "application/json"
    multipart_body: bytes | None = None
    supplied_request_id: str | None = None
    success_schema_keys: frozenset[str] | None = None
    accept: str | None = None
    owner_transition: str | None = None
    mint_fresh_download_capability: bool = False
    coverage_limited: bool = False


@dataclass
class DenialObservation:
    operation_id: str
    row_id: str
    scenario: str
    expected_statuses: list[int]
    actual_status: int
    body_sha256: str
    request_id: str | None
    leaked_markers: list[str]
    owner_transition: str | None = None
    coverage_limited: bool = False


@dataclass
class DenialExecutionReport:
    schema_version: int
    git_sha_full: str
    manifest_sha256: str
    challenge: str
    executable_http_sse_count: int
    observations: list[DenialObservation] = field(default_factory=list)
    failures: list[str] = field(default_factory=list)
    leakage_count: int = 0
    redaction_scan: dict[str, Any] = field(default_factory=lambda: {"passed": True, "findings": []})

    def as_dict(self) -> dict[str, Any]:
        return {
            "schemaVersion": self.schema_version,
            "gitShaFull": self.git_sha_full,
            "manifestSha256": self.manifest_sha256,
            "challenge": self.challenge,
            "executableHttpSseCount": self.executable_http_sse_count,
            "observations": [
                {
                    "operationId": item.operation_id,
                    "rowId": item.row_id,
                    "scenario": item.scenario,
                    "expectedStatuses": item.expected_statuses,
                    "actualStatus": item.actual_status,
                    "bodySha256": item.body_sha256,
                    "requestId": item.request_id,
                    "leakedMarkers": item.leaked_markers,
                    "ownerTransition": item.owner_transition,
                    "coverageLimited": item.coverage_limited,
                }
                for item in self.observations
            ],
            "coverageLimited": sorted(
                {
                    f"{item.row_id}:{item.scenario}"
                    for item in self.observations
                    if item.coverage_limited
                }
            ),
            "failures": sorted(self.failures),
            "leakageCount": self.leakage_count,
            "redactionScan": self.redaction_scan,
        }


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_text(value: str) -> str:
    return sha256_bytes(value.encode("utf-8"))


def validate_uuid(value: str, *, field: str) -> str:
    normalized = value.strip().lower()
    if not UUID_RE.fullmatch(normalized):
        raise RuntimeError(f"{field} must be UUID")
    return normalized


def extract_server_request_id(body: str, headers: dict[str, str]) -> str | None:
    for key, value in headers.items():
        if key.lower() == "x-request-id" and isinstance(value, str) and value.strip():
            candidate = value.strip()
            if UUID_RE.fullmatch(candidate):
                return candidate.lower()
    try:
        payload = json.loads(body)
    except json.JSONDecodeError:
        return None
    if isinstance(payload, dict):
        request_id = payload.get("requestId") or payload.get("request_id")
        if isinstance(request_id, str) and UUID_RE.fullmatch(request_id.strip()):
            return request_id.strip().lower()
    return None


def validate_server_request_id(*, body: str, headers: dict[str, str]) -> str:
    request_id = extract_server_request_id(body, headers)
    if not request_id:
        raise RuntimeError("missing server-minted request id")
    return validate_uuid(request_id, field="requestId")


def canonical_manifest_sha256(manifest_path: Path = DEFAULT_MANIFEST) -> str:
    return sha256_bytes(manifest_path.read_bytes())


def load_manifest_rows(manifest_path: Path = DEFAULT_MANIFEST) -> list[ManifestRow]:
    raw = json.loads(manifest_path.read_text(encoding="utf-8"))
    rows: list[ManifestRow] = []
    for item in raw.get("rows") or []:
        if not isinstance(item, dict):
            continue
        if item.get("status") != "executable":
            continue
        layer = str(item.get("layer") or "")
        if layer not in {"http", "sse"}:
            continue
        operation_id = item.get("operationId")
        test_name = item.get("testName")
        binary = item.get("binary")
        row_id = item.get("id")
        evidence_role = str(item.get("evidenceRole") or "primary")
        if not all(isinstance(value, str) and value.strip() for value in (operation_id, test_name, binary, row_id)):
            raise RuntimeError(f"manifest row missing executable HTTP/SSE fields: {row_id!r}")
        rows.append(
            ManifestRow(
                row_id=row_id,
                operation_id=operation_id,
                layer=layer,
                test_name=test_name,
                binary=binary,
                evidence_role=evidence_role,
            )
        )
    if len(rows) != CANONICAL_EXECUTABLE_HTTP_SSE_COUNT:
        raise RuntimeError(
            f"manifest executable HTTP/SSE row count must be {CANONICAL_EXECUTABLE_HTTP_SSE_COUNT}, saw {len(rows)}"
        )
    return rows


def load_guard_routes(guard_path: Path = DEFAULT_GUARD_INVENTORY) -> dict[str, GuardRoute]:
    raw = json.loads(guard_path.read_text(encoding="utf-8"))
    routes: dict[str, GuardRoute] = {}
    for item in raw.get("operations") or []:
        if not isinstance(item, dict):
            continue
        operation_id = item.get("operationId")
        route = item.get("route") or {}
        method = str(route.get("method") or "").upper()
        path_template = str(route.get("path") or "")
        authz_kind = str(item.get("authzKind") or "")
        if not isinstance(operation_id, str) or not operation_id.strip():
            continue
        if not method or not path_template:
            raise RuntimeError(f"guard inventory missing route for {operation_id}")
        if operation_id in routes:
            raise RuntimeError(f"duplicate guard inventory operationId {operation_id}")
        routes[operation_id] = GuardRoute(
            operation_id=operation_id,
            method=method,
            path_template=path_template,
            authz_kind=authz_kind,
        )
    return routes


def build_http_sse_denial_mapping(
    *,
    manifest_path: Path = DEFAULT_MANIFEST,
    guard_path: Path = DEFAULT_GUARD_INVENTORY,
) -> list[DenialMappingEntry]:
    rows = load_manifest_rows(manifest_path)
    guard = load_guard_routes(guard_path)
    mapping: list[DenialMappingEntry] = []
    seen_rows: set[str] = set()
    for row in rows:
        if row.row_id in seen_rows:
            raise RuntimeError(f"duplicate manifest row id {row.row_id}")
        seen_rows.add(row.row_id)
        guard_route = guard.get(row.operation_id)
        if guard_route is None:
            raise RuntimeError(f"missing guard inventory mapping for operationId {row.operation_id}")
        mapping.append(
            DenialMappingEntry(
                row_id=row.row_id,
                operation_id=row.operation_id,
                layer=row.layer,
                method=guard_route.method,
                path_template=guard_route.path_template,
                authz_kind=guard_route.authz_kind,
                test_name=row.test_name,
                binary=row.binary,
                evidence_role=row.evidence_role,
            )
        )
    if len(mapping) != CANONICAL_EXECUTABLE_HTTP_SSE_COUNT:
        raise RuntimeError("incomplete HTTP/SSE denial mapping")
    return mapping


def validate_revision_binding(
    *,
    source_revision: dict[str, Any],
    manifest_sha256: str,
    git_sha_full: str,
    manifest_path: Path = DEFAULT_MANIFEST,
) -> None:
    commit = source_revision.get("commit")
    if not isinstance(commit, str) or not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise RuntimeError("sourceRevision.commit must be 40-char git SHA")
    if source_revision.get("dirty") is not False:
        raise RuntimeError("sourceRevision.dirty must be false")
    if commit != git_sha_full:
        raise RuntimeError("sourceRevision.commit mismatch with gitShaFull")
    canonical = canonical_manifest_sha256(manifest_path)
    if manifest_sha256 != canonical:
        raise RuntimeError("manifestSha256 mismatch with canonical manifest bytes")


def _substitute_path(template: str, params: dict[str, str]) -> str:
    path = template
    for key, value in params.items():
        path = path.replace("{" + key + "}", value)
    if "{" in path or "}" in path:
        raise RuntimeError(f"unresolved path template placeholders remain in {path!r}")
    return API_PREFIX + path


def _optional_seed_string(seed: Any, key: str, *, fallback: str) -> str:
    value = getattr(seed, key, None)
    if isinstance(value, str) and value.strip():
        return value.strip()
    return fallback


def _credential_string(credentials: Any, key: str, *, field: str) -> str:
    value = getattr(credentials, key, None)
    if isinstance(value, str) and value.strip():
        return value.strip()
    raise RuntimeError(f"credentials missing {field}")


def _resolve_citation_body(seed: Any, *, credentials: Any, foreign: bool) -> dict[str, Any]:
    document_id = seed.beta_document_id if foreign else seed.beta_document_id
    version_id = seed.beta_version_id
    return {
        "logicalDocumentId": document_id,
        "versionId": version_id,
        "sourceContentSha256": _credential_string(
            credentials, "beta_citation_source_content_sha256", field="betaCitationSourceContentSha256"
        ),
        "canonicalMarkdownSha256": _credential_string(
            credentials, "beta_citation_canonical_markdown_sha256", field="betaCitationCanonicalMarkdownSha256"
        ),
        "chunkId": _credential_string(credentials, "beta_citation_chunk_id", field="betaCitationChunkId"),
        "sourceSpanStart": int(getattr(credentials, "beta_citation_source_span_start", 0) or 0),
        "sourceSpanEnd": int(getattr(credentials, "beta_citation_source_span_end", 0) or 0),
        "quoteLocalStart": int(getattr(credentials, "beta_citation_quote_local_start", 0) or 0),
        "quoteLocalEnd": int(getattr(credentials, "beta_citation_quote_local_end", 0) or 0),
        "quote": _credential_string(credentials, "beta_citation_quote", field="betaCitationQuote"),
    }


def _owner_token(entry: DenialMappingEntry, *, seed: Any, credentials: Any) -> str | None:
    if entry.operation_id == "acceptMemberInvite":
        return _credential_string(
            credentials, "beta_denial_accept_access_token", field="betaDenialAcceptAccessToken"
        )
    if entry.operation_id == "redeemDownload":
        return None
    if entry.operation_id in {"patchMember", "deleteMember"} and entry.path_template.endswith("{userId}"):
        params = _owner_params(seed, credentials=credentials, operation_id=entry.operation_id)
        patch_member = _credential_string(
            credentials, "beta_denial_disposable_member_user_id", field="betaDenialDisposableMemberUserId"
        )
        delete_member = _credential_string(
            credentials,
            "beta_denial_disposable_delete_member_user_id",
            field="betaDenialDisposableDeleteMemberUserId",
        )
        if params.get("userId") == patch_member:
            return _credential_string(credentials, "alpha_beta_access_token", field="alphaBetaAccessToken")
        if params.get("userId") == delete_member:
            return _credential_string(credentials, "alpha_beta_access_token", field="alphaBetaAccessToken")
        return _credential_string(credentials, "alpha_access_token", field="alphaAccessToken")
    if entry.operation_id in {"createMemberInvite", "revokeMemberInvite"}:
        return _credential_string(credentials, "alpha_beta_access_token", field="alphaBetaAccessToken")
    return _credential_string(credentials, "alpha_beta_access_token", field="alphaBetaAccessToken")


def _foreign_params(seed: Any, *, credentials: Any) -> dict[str, str]:
    return {
        "orgId": seed.org_beta_id,
        "collectionId": seed.beta_collection_id,
        "documentId": seed.beta_document_id,
        "jobId": seed.beta_job_id,
        "userId": _credential_string(
            credentials, "beta_denial_disposable_member_user_id", field="betaDenialDisposableMemberUserId"
        ),
        "sessionId": seed.beta_chat_session_id,
        "projectId": seed.beta_project_id,
        "versionId": seed.beta_version_id,
        "conflictId": seed.beta_conflict_id,
        "inviteId": seed.beta_invite_id,
        "capability": credentials.beta_download_capability,
    }


def _owner_params(seed: Any, *, credentials: Any, operation_id: str) -> dict[str, str]:
    params = _foreign_params(seed, credentials=credentials)
    if operation_id in DISPOSABLE_COLLECTION_DELETE_OPS:
        params["collectionId"] = _credential_string(
            credentials, "beta_denial_disposable_collection_id", field="betaDenialDisposableCollectionId"
        )
    if operation_id in DISPOSABLE_COLLECTION_UPDATE_OPS:
        params["collectionId"] = _credential_string(
            credentials,
            "beta_denial_disposable_collection_update_id",
            field="betaDenialDisposableCollectionUpdateId",
        )
    if operation_id in DISPOSABLE_DOCUMENT_OPS:
        params["documentId"] = _credential_string(
            credentials, "beta_denial_disposable_document_id", field="betaDenialDisposableDocumentId"
        )
    if operation_id in DISPOSABLE_CHAT_OPS:
        params["sessionId"] = _credential_string(
            credentials, "beta_denial_disposable_chat_session_id", field="betaDenialDisposableChatSessionId"
        )
    if operation_id in DISPOSABLE_INVITE_OPS:
        params["inviteId"] = _credential_string(
            credentials, "beta_denial_disposable_invite_id", field="betaDenialDisposableInviteId"
        )
    if operation_id in DISPOSABLE_MEMBER_PATCH_OPS:
        params["userId"] = _credential_string(
            credentials, "beta_denial_disposable_member_user_id", field="betaDenialDisposableMemberUserId"
        )
    if operation_id in DISPOSABLE_MEMBER_DELETE_OPS:
        params["userId"] = _credential_string(
            credentials,
            "beta_denial_disposable_delete_member_user_id",
            field="betaDenialDisposableDeleteMemberUserId",
        )
    if operation_id in DISPOSABLE_CONFLICT_OPS:
        params["conflictId"] = _credential_string(
            credentials, "beta_denial_disposable_conflict_id", field="betaDenialDisposableConflictId"
        )
    if operation_id in APPROVE_INTAKE_OPS:
        params["documentId"] = _credential_string(
            credentials, "beta_denial_quarantined_document_id", field="betaDenialQuarantinedDocumentId"
        )
        params["collectionId"] = _credential_string(
            credentials, "beta_denial_quarantined_collection_id", field="betaDenialQuarantinedCollectionId"
        )
    return params


def _disposable_org_id(seed: Any) -> str:
    value = getattr(seed, "disposable_org_id", None)
    if isinstance(value, str) and value.strip():
        return validate_uuid(value, field="disposableOrgId")
    raise RuntimeError("seed missing disposableOrgId")


def _negative_invite_token(credentials: Any) -> str:
    return _credential_string(
        credentials, "beta_denial_negative_invite_token", field="betaDenialNegativeInviteToken"
    )


def _wrong_download_capability(credentials: Any) -> str:
    return _credential_string(
        credentials, "beta_denial_wrong_download_capability", field="betaDenialWrongDownloadCapability"
    )


def _body_for_operation(
    operation_id: str,
    seed: Any,
    *,
    credentials: Any,
    foreign: bool,
) -> dict[str, Any] | None:
    collection_id = seed.beta_collection_id if foreign else seed.alpha_collection_id
    document_id = seed.beta_document_id if foreign else seed.alpha_document_id
    org_id = seed.org_beta_id if foreign else seed.org_alpha_id
    suffix = seed.challenge[:8]
    if operation_id in {"search"}:
        return {"query": f"phase1c-denial-{suffix}", "collectionIds": [collection_id]}
    if operation_id in {"ask"}:
        return {"question": f"phase1c-denial-{suffix}", "collectionIds": [collection_id]}
    if operation_id == "askStream":
        return {"question": f"phase1c-denial-stream-{suffix}", "collectionIds": [collection_id]}
    if operation_id == "switchOrg":
        return {"orgId": _disposable_org_id(seed) if foreign else org_id}
    if operation_id == "createOrg":
        return {"slug": f"denial-probe-{suffix}", "name": "Denial Probe Org"}
    if operation_id == "createCollection":
        return {
            "name": f"denial-{suffix}",
            "slug": f"denial-{suffix}",
            "visibility": "org",
        }
    if operation_id == "createProject":
        return {"name": f"denial-project-{suffix}"}
    if operation_id == "createUpload":
        return {"collectionId": collection_id, "filename": "denial.txt", "contentType": "text/plain"}
    if operation_id == "createMemberInvite":
        return {"email": f"invite-{suffix}@example.com", "role": "viewer"}
    if operation_id == "acceptMemberInvite":
        token_key = "beta_denial_negative_invite_token" if foreign else "beta_denial_accept_invite_token"
        field = "betaDenialNegativeInviteToken" if foreign else "betaDenialAcceptInviteToken"
        return {"token": _credential_string(credentials, token_key, field=field)}
    if operation_id == "resolveCitation":
        return _resolve_citation_body(seed, credentials=credentials, foreign=foreign)
    if operation_id == "appendChatTurn":
        return {
            "question": f"phase1c-{suffix}?",
            "answer": f"phase1c-{suffix}",
            "answerMode": "grounded",
        }
    if operation_id == "createChatSession":
        return {"title": f"phase1c-{suffix}"}
    if operation_id == "updateChatSession":
        return {"title": f"phase1c-updated-{suffix}"}
    if operation_id == "patchMember":
        return {"role": "viewer"}
    if operation_id == "publishDocumentVersion":
        return {}
    if operation_id == "reindexDocument":
        return {}
    if operation_id == "approveIntake":
        return {}
    if operation_id == "assignCollectionProject":
        return {"projectId": seed.beta_project_id if foreign else seed.alpha_project_id}
    if operation_id == "issueDownloadCapability":
        return {"purpose": "markdown"}
    if operation_id == "triageConflict":
        return {"status": "resolved", "resolutionNote": f"phase1c-{suffix}"}
    if operation_id == "updateCollection":
        return {"name": f"denial-updated-{suffix}"}
    if operation_id == "updateProject":
        return {"name": f"denial-project-updated-{suffix}"}
    if operation_id in {"revokeMemberInvite"}:
        return {}
    return None


def _uses_foreign_scope(operation_id: str, path_template: str) -> bool:
    if operation_id in {"createOrg"}:
        return False
    if operation_id == "redeemDownload":
        return True
    if operation_id == "createUpload":
        return True
    if any(
        token in path_template
        for token in (
            "{orgId}",
            "{collectionId}",
            "{documentId}",
            "{jobId}",
            "{userId}",
            "{sessionId}",
            "{projectId}",
            "{versionId}",
            "{conflictId}",
            "{inviteId}",
        )
    ):
        return True
    if operation_id in {
        "search",
        "ask",
        "askStream",
        "assignCollectionProject",
        "resolveCitation",
        "switchOrg",
        "acceptMemberInvite",
    }:
        return True
    return False


def _negative_scenario(operation_id: str) -> str:
    if operation_id == "switchOrg":
        return "membership_missing"
    if operation_id == "redeemDownload":
        return "invalid_capability"
    if operation_id == "acceptMemberInvite":
        return "invalid_invite_token"
    return "foreign"


def _negative_denial_statuses(operation_id: str) -> frozenset[int]:
    return OPERATION_FOREIGN_DENIAL.get(operation_id, DEFAULT_FOREIGN_DENIAL)


def _multipart_upload_body(*, collection_id: str, filename: str, content: bytes, boundary: str) -> bytes:
    parts: list[bytes] = []
    parts.append(
        f'--{boundary}\r\nContent-Disposition: form-data; name="collectionId"\r\n\r\n{collection_id}\r\n'.encode(
            "utf-8"
        )
    )
    parts.append(
        (
            f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{filename}"\r\n'
            f"Content-Type: text/plain\r\n\r\n"
        ).encode("utf-8")
    )
    parts.append(content)
    parts.append(f"\r\n--{boundary}--\r\n".encode("utf-8"))
    return b"".join(parts)


def _owner_body(operation_id: str, seed: Any, *, credentials: Any, params: dict[str, str]) -> dict[str, Any] | None:
    if operation_id == "search":
        return {"query": f"phase1c-owner-{seed.challenge[:8]}", "collectionIds": [seed.beta_collection_id]}
    if operation_id == "ask":
        return {"question": f"phase1c-owner-{seed.challenge[:8]}", "collectionIds": [seed.beta_collection_id]}
    if operation_id == "askStream":
        return {"question": f"phase1c-owner-{seed.challenge[:8]}", "collectionIds": [seed.beta_collection_id]}
    if operation_id == "switchOrg":
        return {"orgId": seed.org_beta_id}
    if operation_id == "createOrg":
        return {"slug": f"owner-{seed.challenge[:8]}", "name": "Owner Probe Org"}
    if operation_id == "createCollection":
        return {
            "name": f"owner-{seed.challenge[:8]}",
            "slug": f"owner-{seed.challenge[:8]}",
            "visibility": "org",
        }
    if operation_id == "createProject":
        return {"name": f"owner-project-{seed.challenge[:8]}"}
    if operation_id == "createMemberInvite":
        return {"email": f"owner-{seed.challenge[:8]}@example.com", "role": "viewer"}
    if operation_id == "acceptMemberInvite":
        return {"token": _credential_string(credentials, "beta_denial_accept_invite_token", field="betaDenialAcceptInviteToken")}
    if operation_id == "resolveCitation":
        return _resolve_citation_body(seed, credentials=credentials, foreign=False)
    if operation_id == "appendChatTurn":
        return {
            "question": f"phase1c-owner-{seed.challenge[:8]}?",
            "answer": f"phase1c-owner-{seed.challenge[:8]}",
            "answerMode": "grounded",
        }
    if operation_id == "createChatSession":
        return {"title": f"phase1c-owner-{seed.challenge[:8]}"}
    if operation_id == "updateChatSession":
        return {"title": f"phase1c-owner-updated-{seed.challenge[:8]}"}
    if operation_id == "patchMember":
        return {"role": "viewer"}
    if operation_id == "assignCollectionProject":
        return {"projectId": seed.beta_project_id}
    if operation_id == "issueDownloadCapability":
        return {"purpose": "markdown"}
    if operation_id == "triageConflict":
        return {"status": "resolved", "resolutionNote": f"phase1c-{seed.challenge[:8]}"}
    if operation_id == "updateCollection":
        return {"name": f"owner-updated-{seed.challenge[:8]}"}
    if operation_id == "updateProject":
        return {"name": f"owner-project-updated-{seed.challenge[:8]}"}
    if operation_id in {"publishDocumentVersion", "reindexDocument", "approveIntake", "revokeMemberInvite"}:
        return {}
    return _body_for_operation(operation_id, seed, credentials=credentials, foreign=False)


def _read_schema(operation_id: str) -> frozenset[str]:
    schema = HTTP_OWNER_SUCCESS_SCHEMA.get(operation_id)
    if schema is None:
        raise RuntimeError(f"HTTP_OWNER_SUCCESS_SCHEMA missing {operation_id}")
    return schema


def _chat_session_view(payload: dict[str, Any]) -> tuple[dict[str, Any], list[Any]]:
    nested = payload.get("session")
    if isinstance(nested, dict):
        turns = payload.get("turns")
        if not isinstance(turns, list):
            raise RuntimeError("getChatSession response missing turns")
        return nested, turns
    turns = payload.get("turns")
    if not isinstance(turns, list):
        raise RuntimeError("getChatSession response missing turns")
    session = {
        key: payload[key]
        for key in ("id", "title", "createdAt", "updatedAt")
        if key in payload
    }
    if not session:
        raise RuntimeError("getChatSession response missing session fields")
    return session, turns


def validate_owner_read_response(
    operation_id: str,
    body: str,
    *,
    seed: Any,
    credentials: Any,
    path: str = "",
) -> None:
    payload = json.loads(body)
    if not isinstance(payload, dict):
        raise RuntimeError(f"{operation_id} owner response must be object")
    if operation_id == "authMe":
        for key, expected in (
            ("userId", getattr(seed, "alpha_user_id", None)),
            ("orgId", seed.org_beta_id),
            ("sessionId", credentials.alpha_session_id),
        ):
            actual = payload.get(key)
            if expected and str(actual) != str(expected):
                raise RuntimeError(f"authMe owner {key} mismatch: expected {expected}, got {actual!r}")
        return
    if operation_id == "getChatSession":
        session, turns = _chat_session_view(payload)
        session_id = path.rstrip("/").split("/")[-1] if path else seed.beta_chat_session_id
        if str(session.get("id")) != str(session_id):
            raise RuntimeError("getChatSession session.id mismatch with requested sessionId")
        if not isinstance(session.get("title"), str) or not str(session["title"]).strip():
            raise RuntimeError("getChatSession session.title missing")
        if not isinstance(turns, list):
            raise RuntimeError("getChatSession turns must be list")
        return
    if operation_id == "getCollection":
        collection_id = path.rstrip("/").split("/")[-1] if "/collections/" in path else seed.beta_collection_id
        if str(payload.get("id")) != str(collection_id):
            raise RuntimeError("getCollection id mismatch with requested collectionId")
        name = payload.get("name")
        if not isinstance(name, str) or not name.strip():
            raise RuntimeError("getCollection name missing")
        disposable_update = getattr(credentials, "beta_denial_disposable_collection_update_id", None)
        if disposable_update and str(collection_id) == str(disposable_update):
            suffix = str(getattr(seed, "challenge", ""))[:8]
            if name == f"owner-updated-{suffix}" or name == f"denial-updated-{suffix}":
                return
            raise RuntimeError("getCollection disposable update name mismatch with fixture")
        if str(name) == str(getattr(seed, "marker_beta", "")):
            return
        beta_dup = getattr(seed, "beta_duplicate_collection_id", "")
        if beta_dup and str(collection_id) == str(beta_dup) and name == DUPLICATE_COLLECTION_NAME:
            return
        if name == DUPLICATE_COLLECTION_NAME:
            return
        raise RuntimeError("getCollection name mismatch with fixture marker")
    if operation_id == "getDocument":
        document_id = path.rstrip("/").split("/")[-1] if "/documents/" in path else seed.beta_document_id
        if str(payload.get("id")) != str(document_id):
            raise RuntimeError("getDocument id mismatch")
        if not isinstance(payload.get("title"), str):
            raise RuntimeError("getDocument title missing")
        return
    if operation_id == "getDocumentVersion":
        if str(payload.get("documentId")) != str(seed.beta_document_id):
            raise RuntimeError("getDocumentVersion documentId mismatch")
        version_id = path.split("/versions/")[1].split("/")[0] if "/versions/" in path else seed.beta_version_id
        if str(payload.get("id")) != str(version_id):
            raise RuntimeError("getDocumentVersion id mismatch")
        return
    if operation_id == "getOrg":
        if str(payload.get("id")) != str(seed.org_beta_id):
            raise RuntimeError("getOrg id mismatch")
        return
    if operation_id == "getJob":
        if str(payload.get("id")) != str(seed.beta_job_id):
            raise RuntimeError("getJob id mismatch")
        return
    if operation_id == "getConflict":
        conflict_id = path.rstrip("/").split("/")[-1] if "/conflicts/" in path else seed.beta_conflict_id
        if str(payload.get("id")) != str(conflict_id):
            raise RuntimeError("getConflict id mismatch")
        return
    if operation_id == "getProject":
        if str(payload.get("id")) != str(seed.beta_project_id):
            raise RuntimeError("getProject id mismatch")
        return
    if operation_id == "previewDocument":
        if str(payload.get("documentId")) != str(seed.beta_document_id):
            raise RuntimeError("previewDocument documentId mismatch")
        if str(payload.get("versionId")) != str(seed.beta_version_id):
            raise RuntimeError("previewDocument versionId mismatch")
        return
    if operation_id == "listCollections":
        items = payload.get("items")
        if not isinstance(items, list):
            raise RuntimeError("listCollections items missing")
        return
    if operation_id == "listMembers":
        items = payload.get("items")
        if not isinstance(items, list):
            raise RuntimeError("listMembers items missing")
        return
    if operation_id == "listMemberInvites":
        items = payload.get("items")
        if not isinstance(items, list):
            raise RuntimeError("listMemberInvites items missing")
        return
    required = HTTP_OWNER_SUCCESS_SCHEMA.get(operation_id)
    if required:
        for key in required:
            if key not in payload and operation_id != "getChatSession":
                raise RuntimeError(f"{operation_id} owner response missing {key}")


def validate_http_contract_schema_table() -> None:
    for operation_id, keys in HTTP_OWNER_SUCCESS_SCHEMA.items():
        if not keys:
            raise RuntimeError(f"HTTP_OWNER_SUCCESS_SCHEMA[{operation_id!r}] must be non-empty")
    mapping = build_http_sse_denial_mapping()
    get_ops = {
        entry.operation_id
        for entry in mapping
        if entry.method.upper() == "GET"
        and entry.operation_id not in {"redeemDownload", "jobEvents"}
        and entry.layer != "sse"
    }
    missing = sorted(get_ops - set(HTTP_OWNER_SUCCESS_SCHEMA))
    if missing:
        raise RuntimeError(f"HTTP_OWNER_SUCCESS_SCHEMA missing GET operations: {missing}")


def build_owner_control_spec(
    entry: DenialMappingEntry,
    *,
    seed: Any,
    credentials: Any,
) -> OwnerControlSpec | None:
    params = _owner_params(seed, credentials=credentials, operation_id=entry.operation_id)
    path = _substitute_path(entry.path_template, params)
    if entry.operation_id == "diffDocumentVersions":
        against = params.get("versionId") or seed.beta_version_id
        path = f"{path}?against={against}"
    method = entry.method.upper()

    owner_token = _owner_token(entry, seed=seed, credentials=credentials)

    if entry.operation_id == "createUpload":
        multipart = _multipart_upload_body(
            collection_id=seed.beta_collection_id,
            filename="owner-denial.txt",
            content=f"phase1c-owner-{seed.challenge}\n".encode("utf-8"),
            boundary=MULTIPART_BOUNDARY,
        )
        return OwnerControlSpec(
            method="POST",
            path=f"{API_PREFIX}/uploads",
            body=None,
            expected_statuses=frozenset({200, 201}),
            content_type=f"multipart/form-data; boundary={MULTIPART_BOUNDARY}",
            multipart_body=multipart,
            success_schema_keys=frozenset({"documentId", "versionId"}),
            token=owner_token,
        )

    if entry.operation_id == "redeemDownload":
        return OwnerControlSpec(
            method="GET",
            path=f"{API_PREFIX}/downloads/{credentials.beta_download_capability}",
            body=None,
            expected_statuses=frozenset({200}),
            token=credentials.beta_access_token,
        )

    if entry.operation_id == "askStream":
        return OwnerControlSpec(
            method="POST",
            path=f"{API_PREFIX}/ask/stream",
            body=_owner_body("askStream", seed, credentials=credentials, params=params),
            expected_statuses=frozenset({200}),
            accept="text/event-stream",
            token=owner_token,
        )

    if entry.operation_id == "jobEvents":
        return OwnerControlSpec(
            method="GET",
            path=path,
            body=None,
            expected_statuses=frozenset({200}),
            accept="text/event-stream",
            token=owner_token,
        )

    if entry.method == "GET":
        return OwnerControlSpec(
            method=method,
            path=path,
            body=None,
            expected_statuses=frozenset({200}),
            success_schema_keys=_read_schema(entry.operation_id),
            token=owner_token,
        )

    body = _owner_body(entry.operation_id, seed, credentials=credentials, params=params)
    expected = frozenset({200, 201, 204})
    schema = None
    if entry.operation_id == "approveIntake":
        expected = frozenset({403, 404})
    if entry.operation_id in {"createCollection", "createProject", "createChatSession", "createOrg"}:
        schema = frozenset({"id"})
    if entry.operation_id == "resolveCitation":
        schema = frozenset({"citation", "requestId"})
    return OwnerControlSpec(
        method=method,
        path=path,
        body=body,
        expected_statuses=expected,
        success_schema_keys=schema,
        token=owner_token,
    )


def _append_owner_spec(
    specs: list[DenialRequestSpec],
    *,
    entry: DenialMappingEntry,
    seed: Any,
    credentials: Any,
    request_id: str,
    owner_transition: str | None = None,
) -> None:
    owner = build_owner_control_spec(entry, seed=seed, credentials=credentials)
    if owner is None:
        return
    _append_owner_control_from_spec(
        specs,
        row_id=entry.row_id,
        operation_id=entry.operation_id,
        owner=owner,
        request_id=request_id,
        suffix="owner",
        credentials=credentials,
        owner_transition=owner_transition,
        coverage_limited=entry.operation_id == "approveIntake",
    )


def _append_owner_control_from_spec(
    specs: list[DenialRequestSpec],
    *,
    row_id: str,
    operation_id: str,
    owner: OwnerControlSpec,
    request_id: str,
    suffix: str,
    credentials: Any,
    owner_transition: str | None = None,
    body: dict[str, Any] | None = None,
    coverage_limited: bool = False,
) -> None:
    token = owner.token if owner.token is not None else credentials.alpha_beta_access_token
    specs.append(
        DenialRequestSpec(
            row_id=row_id,
            operation_id=operation_id,
            scenario="owner_control",
            method=owner.method,
            path=owner.path,
            token=token,
            body=body if body is not None else owner.body,
            expected_statuses=owner.expected_statuses,
            content_type=owner.content_type,
            multipart_body=owner.multipart_body,
            supplied_request_id=f"{request_id}-{suffix}",
            success_schema_keys=owner.success_schema_keys,
            accept=owner.accept,
            owner_transition=owner_transition,
            coverage_limited=coverage_limited,
        )
    )


def _clone_spec_as_owner_control(spec: DenialRequestSpec, *, suffix: str) -> DenialRequestSpec:
    base_id = spec.supplied_request_id or f"phase1c-{spec.row_id}"
    return DenialRequestSpec(
        row_id=spec.row_id,
        operation_id=spec.operation_id,
        scenario="owner_control",
        method=spec.method,
        path=spec.path,
        token=spec.token,
        body=spec.body,
        expected_statuses=spec.expected_statuses,
        content_type=spec.content_type,
        multipart_body=spec.multipart_body,
        supplied_request_id=f"{base_id}-{suffix}",
        success_schema_keys=spec.success_schema_keys,
        accept=spec.accept,
        owner_transition=spec.owner_transition,
    )


def _build_primary_row_specs(
    entry: DenialMappingEntry,
    *,
    seed: Any,
    credentials: Any,
) -> list[DenialRequestSpec]:
    specs: list[DenialRequestSpec] = []
    params = _foreign_params(seed, credentials=credentials)
    path = _substitute_path(entry.path_template, params)
    body = _body_for_operation(entry.operation_id, seed, credentials=credentials, foreign=True)
    request_id = f"phase1c-{entry.row_id}-{secrets.token_hex(4)}"
    owner_transition = {
        "deleteCollection": "collection_deleted",
        "updateCollection": "collection_updated",
        "deleteDocument": "document_deleted",
        "deleteChatSession": "chat_session_deleted",
        "patchMember": "member_role_updated",
        "deleteMember": "member_deleted",
        "revokeMemberInvite": "invite_revoked",
        "triageConflict": "conflict_triaged",
        "acceptMemberInvite": "invite_accepted",
    }.get(entry.operation_id)

    upload_multipart = None
    content_type = "application/json"
    if entry.operation_id == "createUpload":
        upload_multipart = _multipart_upload_body(
            collection_id=seed.beta_collection_id,
            filename="denial.txt",
            content=f"phase1c-denial-{seed.challenge}\n".encode("utf-8"),
            boundary=MULTIPART_BOUNDARY,
        )
        content_type = f"multipart/form-data; boundary={MULTIPART_BOUNDARY}"
        body = None

    if _uses_foreign_scope(entry.operation_id, entry.path_template) or entry.operation_id in CURRENT_ORG_READ_OPS:
        _append_owner_spec(
            specs,
            entry=entry,
            seed=seed,
            credentials=credentials,
            request_id=request_id,
            owner_transition=owner_transition,
        )

    if entry.authz_kind != "public":
        specs.append(
            DenialRequestSpec(
                row_id=entry.row_id,
                operation_id=entry.operation_id,
                scenario="unauthenticated",
                method=entry.method,
                path=path,
                token=None,
                body=body,
                expected_statuses=frozenset({401}),
                content_type=content_type,
                multipart_body=upload_multipart,
                supplied_request_id=f"{request_id}-unauth",
            )
        )

    if _uses_foreign_scope(entry.operation_id, entry.path_template):
        negative_path = path
        negative_token = credentials.alpha_access_token
        negative_body = body
        negative_scenario = _negative_scenario(entry.operation_id)
        if entry.operation_id == "redeemDownload":
            wrong_cap = _wrong_download_capability(credentials)
            negative_path = f"{API_PREFIX}/downloads/{wrong_cap}"
            negative_token = credentials.beta_access_token
            negative_body = None
        elif entry.operation_id == "acceptMemberInvite":
            negative_token = _credential_string(
                credentials, "beta_denial_accept_access_token", field="betaDenialAcceptAccessToken"
            )
            negative_body = {"token": _negative_invite_token(credentials)}
        specs.append(
            DenialRequestSpec(
                row_id=entry.row_id,
                operation_id=entry.operation_id,
                scenario=negative_scenario,
                method=entry.method,
                path=negative_path,
                token=negative_token,
                body=negative_body,
                expected_statuses=_negative_denial_statuses(entry.operation_id),
                content_type=content_type,
                multipart_body=upload_multipart,
                supplied_request_id=f"{request_id}-negative",
            )
        )
        if entry.operation_id == "acceptMemberInvite":
            already_token = getattr(credentials, "beta_denial_already_member_invite_token", "")
            if isinstance(already_token, str) and already_token.strip():
                specs.append(
                    DenialRequestSpec(
                        row_id=entry.row_id,
                        operation_id=entry.operation_id,
                        scenario="invite_already_member",
                        method=entry.method,
                        path=negative_path,
                        token=_credential_string(
                            credentials, "beta_denial_accept_access_token", field="betaDenialAcceptAccessToken"
                        ),
                        body={"token": already_token},
                        expected_statuses=frozenset({409}),
                        content_type=content_type,
                        supplied_request_id=f"{request_id}-already-member",
                    )
                )
    if entry.operation_id in CURRENT_ORG_READ_OPS:
        # Collection-scoped list ops use foreign ids in the path; production returns
        # not-found when the collection is outside the authenticated org context
        # (documents.rs:195-197), not 200 with current-org data.
        foreign_isolation_statuses = frozenset({200})
        if entry.operation_id == "listDocuments":
            foreign_isolation_statuses = frozenset({404})
        specs.append(
            DenialRequestSpec(
                row_id=entry.row_id,
                operation_id=entry.operation_id,
                scenario="authenticated_foreign_isolation",
                method=entry.method,
                path=path if entry.operation_id in CURRENT_ORG_READ_OPS else path,
                token=credentials.alpha_access_token,
                body=body if entry.method.upper() != "GET" else None,
                expected_statuses=foreign_isolation_statuses,
                content_type=content_type,
                multipart_body=upload_multipart,
                supplied_request_id=f"{request_id}-auth-foreign-isolation",
            )
        )
    return specs


def _handler_citation_matrix(entry: DenialMappingEntry, seed: Any, credentials: Any) -> list[DenialRequestSpec]:
    specs = _build_primary_row_specs(entry, seed=seed, credentials=credentials)
    request_id = f"phase1c-{entry.row_id}-{secrets.token_hex(4)}"
    owner = build_owner_control_spec(entry, seed=seed, credentials=credentials)
    if owner is not None:
        repeat_body = dict(owner.body or {})
        specs.append(
            DenialRequestSpec(
                row_id=entry.row_id,
                operation_id=entry.operation_id,
                scenario="citation_repeat",
                method=owner.method,
                path=owner.path,
                token=owner.token,
                body=repeat_body,
                expected_statuses=frozenset({200}),
                supplied_request_id=f"{request_id}-repeat",
                coverage_limited=True,
            )
        )
        expired_body = dict(owner.body or {})
        expired_version = getattr(credentials, "beta_citation_expired_version_id", "")
        if isinstance(expired_version, str) and expired_version.strip():
            expired_body["versionId"] = expired_version
            expired_body["requireCurrent"] = True
        specs.append(
            DenialRequestSpec(
                row_id=entry.row_id,
                operation_id=entry.operation_id,
                scenario="citation_expired",
                method=owner.method,
                path=owner.path,
                token=owner.token,
                body=expired_body,
                expected_statuses=frozenset({404}),
                supplied_request_id=f"{request_id}-expired",
                coverage_limited=True,
            )
        )
        mismatch = dict(owner.body or {})
        mismatch["logicalDocumentId"] = seed.alpha_document_id
        specs.append(
            DenialRequestSpec(
                row_id=entry.row_id,
                operation_id=entry.operation_id,
                scenario="citation_mismatch",
                method=owner.method,
                path=owner.path,
                token=credentials.alpha_access_token,
                body=mismatch,
                expected_statuses=frozenset({404}),
                supplied_request_id=f"{request_id}-mismatch",
            )
        )
    return specs


def _handler_citation_preview(entry: DenialMappingEntry, seed: Any, credentials: Any) -> list[DenialRequestSpec]:
    return _build_primary_row_specs(entry, seed=seed, credentials=credentials)


def _handler_indexed_fts(entry: DenialMappingEntry, seed: Any, credentials: Any) -> list[DenialRequestSpec]:
    specs = _build_primary_row_specs(entry, seed=seed, credentials=credentials)
    for spec in specs:
        if spec.body is not None and spec.operation_id == "ask":
            spec.body = {"question": seed.marker_beta, "collectionIds": [seed.beta_collection_id]}
    return specs


def _handler_duplicate_names(entry: DenialMappingEntry, seed: Any, credentials: Any) -> list[DenialRequestSpec]:
    specs = _build_primary_row_specs(entry, seed=seed, credentials=credentials)
    request_id = f"phase1c-{entry.row_id}-{secrets.token_hex(4)}"
    owner = build_owner_control_spec(entry, seed=seed, credentials=credentials)
    if owner is not None:
        lookup_owner = OwnerControlSpec(
            method="GET",
            path=f"{API_PREFIX}/collections",
            body=None,
            expected_statuses=frozenset({200}),
            success_schema_keys=frozenset({"items"}),
            token=owner.token,
        )
        _append_owner_control_from_spec(
            specs,
            row_id=entry.row_id,
            operation_id="listCollections",
            owner=lookup_owner,
            request_id=request_id,
            suffix="lookup-owner",
            credentials=credentials,
        )
        specs.append(
            DenialRequestSpec(
                row_id=entry.row_id,
                operation_id="listCollections",
                scenario="duplicate_name_owner_lookup",
                method="GET",
                path=f"{API_PREFIX}/collections",
                token=owner.token,
                body=None,
                expected_statuses=frozenset({200}),
                supplied_request_id=f"{request_id}-owner-lookup",
                success_schema_keys=frozenset({"items"}),
            )
        )
        oracle_owner = OwnerControlSpec(
            method="GET",
            path=f"{API_PREFIX}/collections/{getattr(seed, 'beta_duplicate_collection_id', seed.beta_collection_id)}",
            body=None,
            expected_statuses=frozenset({200}),
            success_schema_keys=frozenset({"id", "name"}),
            token=owner.token,
        )
        _append_owner_control_from_spec(
            specs,
            row_id=entry.row_id,
            operation_id="getCollection",
            owner=oracle_owner,
            request_id=request_id,
            suffix="oracle-owner",
            credentials=credentials,
        )
    specs.append(
        DenialRequestSpec(
            row_id=entry.row_id,
            operation_id=entry.operation_id,
            scenario="duplicate_name_foreign_oracle",
            method="GET",
            path=f"{API_PREFIX}/collections/{getattr(seed, 'alpha_duplicate_collection_id', seed.alpha_collection_id)}",
            token=credentials.alpha_beta_access_token,
            body=None,
            expected_statuses=frozenset({403, 404}),
            supplied_request_id=f"{request_id}-foreign-oracle",
        )
    )
    return specs


def _handler_stale_tokens(entry: DenialMappingEntry, seed: Any, credentials: Any) -> list[DenialRequestSpec]:
    specs = _build_primary_row_specs(entry, seed=seed, credentials=credentials)
    owner = build_owner_control_spec(entry, seed=seed, credentials=credentials)
    if owner is not None and not any(spec.scenario == "owner_control" for spec in specs):
        specs.append(
            DenialRequestSpec(
                row_id=entry.row_id,
                operation_id=entry.operation_id,
                scenario="owner_control",
                method=owner.method,
                path=owner.path,
                token=owner.token,
                body=owner.body,
                expected_statuses=owner.expected_statuses,
                success_schema_keys=owner.success_schema_keys,
            )
        )
    request_id = f"phase1c-{entry.row_id}-{secrets.token_hex(4)}"
    stale_specs = [
        ("stale_token", "beta_denial_stale_access_token", "betaDenialStaleAccessToken"),
        ("stale_token_after_downgrade", "beta_denial_stale_after_downgrade_token", "betaDenialStaleAfterDowngradeToken"),
        ("stale_token_after_remove", "beta_denial_stale_after_remove_token", "betaDenialStaleAfterRemoveToken"),
    ]
    for scenario, attr, field in stale_specs:
        token_value = getattr(credentials, attr, "")
        if isinstance(token_value, str) and token_value.strip():
            specs.append(
                DenialRequestSpec(
                    row_id=entry.row_id,
                    operation_id="authMe",
                    scenario=scenario,
                    method="GET",
                    path=f"{API_PREFIX}/auth/me",
                    token=token_value,
                    body=None,
                    expected_statuses=frozenset({401}),
                    supplied_request_id=f"{request_id}-{scenario}-{secrets.token_hex(2)}",
                )
            )
    return specs


def _handler_preview_download_sse(entry: DenialMappingEntry, seed: Any, credentials: Any) -> list[DenialRequestSpec]:
    specs = _build_primary_row_specs(entry, seed=seed, credentials=credentials)
    request_id = f"phase1c-{entry.row_id}-{secrets.token_hex(4)}"
    token = credentials.alpha_beta_access_token
    foreign_token = credentials.alpha_access_token
    preview_path = f"{API_PREFIX}/documents/{seed.beta_document_id}/preview"
    capability_path = (
        f"{API_PREFIX}/documents/{seed.beta_document_id}/versions/{seed.beta_version_id}/download-capability"
    )
    preview_specs = [
            DenialRequestSpec(
                row_id=entry.row_id,
                operation_id="previewDocument",
                scenario="preview_download_preview",
                method="GET",
                path=preview_path,
                token=token,
                body=None,
                expected_statuses=frozenset({200}),
                success_schema_keys=frozenset({"documentId", "versionId"}),
                supplied_request_id=f"{request_id}-preview",
            ),
            DenialRequestSpec(
                row_id=entry.row_id,
                operation_id="previewDocument",
                scenario="preview_download_preview_foreign",
                method="GET",
                path=preview_path,
                token=foreign_token,
                body=None,
                expected_statuses=frozenset({403, 404}),
                supplied_request_id=f"{request_id}-preview-foreign",
            ),
            DenialRequestSpec(
                row_id=entry.row_id,
                operation_id="issueDownloadCapability",
                scenario="preview_download_capability",
                method="POST",
                path=capability_path,
                token=token,
                body={"purpose": "markdown"},
                expected_statuses=frozenset({200, 201}),
                supplied_request_id=f"{request_id}-capability",
            ),
            DenialRequestSpec(
                row_id=entry.row_id,
                operation_id="redeemDownload",
                scenario="preview_download_redeem",
                method="GET",
                path=f"{API_PREFIX}/downloads/{credentials.beta_download_capability}",
                token=token,
                body=None,
                expected_statuses=frozenset({200}),
                supplied_request_id=f"{request_id}-redeem",
                mint_fresh_download_capability=True,
            ),
            DenialRequestSpec(
                row_id=entry.row_id,
                operation_id="getJob",
                scenario="preview_download_job",
                method="GET",
                path=f"{API_PREFIX}/jobs/{seed.beta_job_id}",
                token=token,
                body=None,
                expected_statuses=frozenset({200}),
                success_schema_keys=frozenset({"id", "status"}),
                supplied_request_id=f"{request_id}-job",
            ),
            DenialRequestSpec(
                row_id=entry.row_id,
                operation_id="jobEvents",
                scenario="preview_download_sse",
                method="GET",
                path=f"{API_PREFIX}/jobs/{seed.beta_job_id}/events",
                token=token,
                body=None,
                expected_statuses=frozenset({200}),
                accept="text/event-stream",
                supplied_request_id=f"{request_id}-sse",
            ),
        ]
    for preview_spec in preview_specs:
        specs.append(preview_spec)
    return specs


def _handler_in_flight_revoke(entry: DenialMappingEntry, seed: Any, credentials: Any) -> list[DenialRequestSpec]:
    specs = _build_primary_row_specs(entry, seed=seed, credentials=credentials)
    request_id = f"phase1c-{entry.row_id}-{secrets.token_hex(4)}"
    stream_token = credentials.beta_alpha_access_token
    for spec in specs:
        if spec.operation_id == "askStream" and spec.scenario == "owner_control":
            spec.token = stream_token
            spec.owner_transition = "ask_stream_started"
        if spec.operation_id == "askStream" and spec.scenario == "foreign":
            spec.body = {
                "question": f"phase1c-inflight-foreign-{seed.challenge[:8]}",
                "collectionIds": [seed.alpha_collection_id],
            }
    specs.append(
        DenialRequestSpec(
            row_id=entry.row_id,
            operation_id="deleteMember",
            scenario="in_flight_membership_remove",
            method="DELETE",
            path=f"{API_PREFIX}/members/{seed.beta_member_user_id}",
            token=credentials.alpha_access_token,
            body=None,
            expected_statuses=frozenset({204}),
            supplied_request_id=f"{request_id}-remove",
            owner_transition="ask_stream_revoked",
            coverage_limited=True,
        )
    )
    return specs


ROW_SCENARIO_HANDLERS: dict[str, Callable[..., list[DenialRequestSpec]]] = {
    "denial-resolveCitation-citation": _handler_citation_matrix,
    "denial-previewDocument-citation": _handler_citation_preview,
    "denial-task13-indexed-fts-ask": _handler_indexed_fts,
    "denial-task13-duplicate-names": _handler_duplicate_names,
    "denial-task13-stale-tokens": _handler_stale_tokens,
    "denial-task13-preview-download-sse": _handler_preview_download_sse,
    "denial-task13-in-flight-ask-revoke": _handler_in_flight_revoke,
}


def build_row_denial_specs(
    entry: DenialMappingEntry,
    *,
    seed: Any,
    credentials: Any,
) -> list[DenialRequestSpec]:
    if entry.row_id in SECONDARY_ROW_IDS:
        handler = ROW_SCENARIO_HANDLERS.get(entry.row_id)
        if handler is None or handler is _build_primary_row_specs:
            raise RuntimeError(f"secondary row {entry.row_id} requires dedicated scenario handler")
        return handler(entry, seed, credentials)
    return _build_primary_row_specs(entry, seed=seed, credentials=credentials)


def build_denial_request_specs(
    mapping: list[DenialMappingEntry],
    *,
    seed: Any,
    credentials: Any,
) -> list[DenialRequestSpec]:
    specs: list[DenialRequestSpec] = []
    for entry in mapping:
        specs.extend(build_row_denial_specs(entry, seed=seed, credentials=credentials))
    return specs


def scan_marker_leakage(body: str, *, forbidden_markers: set[str]) -> list[str]:
    leaks: list[str] = []
    for marker in sorted(forbidden_markers):
        if marker and marker in body:
            leaks.append(marker)
    return leaks


def validate_denial_observation_matrix(observations: list[DenialObservation]) -> None:
    by_row: dict[str, list[DenialObservation]] = {}
    for item in observations:
        by_row.setdefault(item.row_id, []).append(item)
    for row_id, rows in by_row.items():
        negatives = [
            row for row in rows if row.scenario not in {"owner_control", "unauthenticated"}
        ]
        owners = [row for row in rows if row.scenario == "owner_control"]
        owners_for_shim = [row for row in owners if not row.coverage_limited]
        if negatives and owners_for_shim:
            if all(row.actual_status == 403 for row in negatives) and all(
                row.actual_status == 403 for row in owners_for_shim
            ):
                operation_id = negatives[0].operation_id
                raise RuntimeError(f"all-403 shim detected for {operation_id}/{row_id}")
        if negatives and not owners:
            raise RuntimeError(f"negative rows missing owner_control: {row_id}")


def _validate_success_schema(body: str, required_keys: frozenset[str]) -> None:
    payload = json.loads(body)
    if not isinstance(payload, dict):
        raise RuntimeError("owner control response must be object")
    for key in required_keys:
        if key not in payload:
            raise RuntimeError(f"owner control response missing {key}")


def parse_sse_stream(
    body: str,
    *,
    required_terminal: str | None = None,
    max_bytes: int = SSE_MAX_BYTES,
    expected_request_id: str | None = None,
) -> list[dict[str, Any]]:
    if len(body.encode("utf-8")) > max_bytes:
        raise RuntimeError("SSE stream exceeds bounded size")
    if not body.strip():
        raise RuntimeError("SSE stream empty")
    events: list[dict[str, Any]] = []
    event_name = ""
    data_lines: list[str] = []
    event_id = ""
    terminal_seen = False
    last_sequence = 0
    for raw_line in body.splitlines():
        line = raw_line.rstrip("\r")
        if not line:
            if event_name or data_lines:
                payload_text = "\n".join(data_lines)
                payload: Any = {}
                if payload_text:
                    try:
                        payload = json.loads(payload_text)
                    except json.JSONDecodeError as error:
                        raise RuntimeError(f"SSE data must be JSON: {error}") from error
                if isinstance(payload, dict):
                    missing = SSE_ENVELOPE_REQUIRED_KEYS - set(payload.keys())
                    if missing and event_name not in {"", "message", "heartbeat"}:
                        raise RuntimeError(f"SSE envelope missing keys: {sorted(missing)}")
                    seq = payload.get("sequence")
                    if isinstance(seq, int):
                        if seq <= last_sequence:
                            raise RuntimeError("SSE sequence must be monotonic")
                        last_sequence = seq
                    env_event = payload.get("event")
                    if isinstance(env_event, str) and event_name and env_event != event_name:
                        raise RuntimeError("SSE event/data event field mismatch")
                    req_id = payload.get("requestId")
                    if expected_request_id and isinstance(req_id, str) and req_id != expected_request_id:
                        raise RuntimeError("SSE requestId mismatch")
                events.append({"event": event_name or "message", "id": event_id, "data": payload})
                if required_terminal and (event_name or "message") == required_terminal:
                    terminal_seen = True
            event_name = ""
            event_id = ""
            data_lines = []
            continue
        if line.startswith("event:"):
            event_name = line.split(":", 1)[1].strip()
        elif line.startswith("id:"):
            event_id = line.split(":", 1)[1].strip()
        elif line.startswith("data:"):
            data_lines.append(line.split(":", 1)[1].strip())
        elif line.startswith(":"):
            continue
        else:
            raise RuntimeError("SSE frame malformed")
    if required_terminal and not terminal_seen:
        raise RuntimeError(f"SSE missing terminal event {required_terminal}")
    if not events:
        raise RuntimeError("SSE response missing frames")
    return events


def validate_sse_envelope(
    body: str,
    *,
    operation_id: str,
    headers: dict[str, str],
    expected_request_id: str | None = None,
) -> None:
    content_type = ""
    for key, value in headers.items():
        if key.lower() == "content-type":
            content_type = value.lower()
            break
    if "text/event-stream" not in content_type:
        raise RuntimeError(f"{operation_id} SSE response missing text/event-stream content-type")
    parse_sse_stream(
        body,
        required_terminal=SSE_TERMINAL_EVENT,
        max_bytes=SSE_MAX_BYTES,
        expected_request_id=expected_request_id,
    )


def _validate_sse_envelope(body: str, *, operation_id: str, headers: dict[str, str]) -> None:
    validate_sse_envelope(
        body,
        operation_id=operation_id,
        headers=headers,
        expected_request_id=None,
    )
    _ = SSE_TERMINAL_EVENT


def _list_members(http_request: HttpRequestFn, *, api_base: str, token: str) -> list[dict[str, Any]]:
    members_path = "/api/v1/members"
    follow = http_request(
        method="GET",
        url=api_base.rstrip("/") + members_path,
        token=token,
        body=None,
        path=members_path,
    )
    if follow.status != 200:
        raise RuntimeError(f"listMembers follow-up expected 200 got {follow.status}")
    payload = json.loads(follow.body)
    items = payload.get("items") or []
    if not isinstance(items, list):
        raise RuntimeError("listMembers items missing")
    return [item for item in items if isinstance(item, dict)]


def _validate_owner_transition(
    spec: DenialRequestSpec,
    *,
    response: HttpResponseLike,
    seed: Any,
    credentials: Any,
    http_request: HttpRequestFn,
    api_base: str,
    transition: str | None,
    report: DenialExecutionReport,
) -> str | None:
    if transition is None:
        return None
    # Member PATCH/DELETE transitions proof via GET /api/v1/members list (no GET /api/v1/members/{id}).
    token = spec.token or credentials.alpha_beta_access_token
    member_id = _credential_string(
        credentials, "beta_denial_disposable_member_user_id", field="betaDenialDisposableMemberUserId"
    )
    delete_member_id = _credential_string(
        credentials,
        "beta_denial_disposable_delete_member_user_id",
        field="betaDenialDisposableDeleteMemberUserId",
    )
    invite_id = _credential_string(
        credentials, "beta_denial_disposable_invite_id", field="betaDenialDisposableInviteId"
    )
    if transition == "collection_deleted":
        follow_up_path = f"{API_PREFIX}/collections/{credentials.beta_denial_disposable_collection_id}"
        follow = http_request(
            method="GET",
            url=api_base.rstrip("/") + follow_up_path,
            token=token,
            body=None,
            path=follow_up_path,
        )
        if follow.status != 404:
            report.failures.append(
                f"{spec.operation_id}/owner_control transition collection_deleted follow-up expected 404 got {follow.status}"
            )
        return transition
    if transition == "collection_updated":
        follow_up_path = f"{API_PREFIX}/collections/{credentials.beta_denial_disposable_collection_update_id}"
        follow = http_request(
            method="GET",
            url=api_base.rstrip("/") + follow_up_path,
            token=token,
            body=None,
            path=follow_up_path,
        )
        if follow.status != 200:
            report.failures.append(
                f"{spec.operation_id}/owner_control transition collection_updated follow-up expected 200 got {follow.status}"
            )
            return transition
        try:
            payload = json.loads(follow.body)
            validate_owner_read_response(
                "getCollection",
                follow.body,
                seed=seed,
                credentials=credentials,
                path=follow_up_path,
            )
        except RuntimeError as error:
            report.failures.append(f"{spec.operation_id}/owner_control transition collection_updated: {error}")
        return transition
    if transition == "document_deleted":
        follow_up_path = f"{API_PREFIX}/documents/{credentials.beta_denial_disposable_document_id}"
        follow = http_request(
            method="GET",
            url=api_base.rstrip("/") + follow_up_path,
            token=token,
            body=None,
            path=follow_up_path,
        )
        if follow.status != 404:
            report.failures.append(
                f"{spec.operation_id}/owner_control transition document_deleted follow-up expected 404 got {follow.status}"
            )
        return transition
    if transition == "chat_session_deleted":
        follow_up_path = f"{API_PREFIX}/chat-sessions/{credentials.beta_denial_disposable_chat_session_id}"
        follow = http_request(
            method="GET",
            url=api_base.rstrip("/") + follow_up_path,
            token=token,
            body=None,
            path=follow_up_path,
        )
        if follow.status != 404:
            report.failures.append(
                f"{spec.operation_id}/owner_control transition chat_session_deleted follow-up expected 404 got {follow.status}"
            )
        return transition
    if transition == "conflict_triaged":
        follow_up_path = f"{API_PREFIX}/conflicts/{credentials.beta_denial_disposable_conflict_id}"
        follow = http_request(
            method="GET",
            url=api_base.rstrip("/") + follow_up_path,
            token=token,
            body=None,
            path=follow_up_path,
        )
        if follow.status != 200:
            report.failures.append(
                f"{spec.operation_id}/owner_control transition conflict_triaged follow-up expected 200 got {follow.status}"
            )
            return transition
        try:
            payload = json.loads(follow.body)
            if payload.get("status") not in {"resolved", "accepted_exception", "false_positive"}:
                report.failures.append(
                    f"{spec.operation_id}/owner_control transition conflict_triaged unexpected status {payload.get('status')!r}"
                )
        except json.JSONDecodeError as error:
            report.failures.append(f"{spec.operation_id}/owner_control transition conflict_triaged invalid json: {error}")
        return transition
    if transition == "member_role_updated":
        try:
            items = _list_members(http_request, api_base=api_base, token=token)
            matched = next((item for item in items if str(item.get("userId")) == member_id), None)
            if matched is None:
                report.failures.append(
                    f"{spec.operation_id}/owner_control transition member_role_updated missing member {member_id}"
                )
            elif matched.get("role") != "viewer":
                report.failures.append(
                    f"{spec.operation_id}/owner_control transition member_role_updated expected role viewer got {matched.get('role')!r}"
                )
        except RuntimeError as error:
            report.failures.append(f"{spec.operation_id}/owner_control transition member_role_updated: {error}")
        return transition
    if transition == "member_deleted":
        try:
            items = _list_members(http_request, api_base=api_base, token=token)
            if any(str(item.get("userId")) == delete_member_id for item in items):
                report.failures.append(
                    f"{spec.operation_id}/owner_control transition member_deleted member still listed"
                )
        except RuntimeError as error:
            report.failures.append(f"{spec.operation_id}/owner_control transition member_deleted: {error}")
        return transition
    if transition == "invite_revoked":
        follow_up_path = f"{API_PREFIX}/members/invites"
        follow = http_request(
            method="GET",
            url=api_base.rstrip("/") + follow_up_path,
            token=token,
            body=None,
            path=follow_up_path,
        )
        if follow.status != 200:
            report.failures.append(
                f"{spec.operation_id}/owner_control transition invite_revoked follow-up expected 200 got {follow.status}"
            )
            return transition
        try:
            payload = json.loads(follow.body)
            items = payload.get("items") or []
            for item in items:
                if isinstance(item, dict) and str(item.get("id")) == invite_id:
                    if item.get("status") not in {"revoked", "expired"}:
                        report.failures.append(
                            f"{spec.operation_id}/owner_control transition invite_revoked invite still active"
                        )
                    break
            else:
                report.failures.append(
                    f"{spec.operation_id}/owner_control transition invite_revoked missing invite {invite_id}"
                )
        except json.JSONDecodeError as error:
            report.failures.append(f"{spec.operation_id}/owner_control transition invite_revoked invalid json: {error}")
        return transition
    if transition == "invite_accepted":
        list_token = _credential_string(credentials, "alpha_beta_access_token", field="alphaBetaAccessToken")
        try:
            items = _list_members(http_request, api_base=api_base, token=list_token)
            accept_user = "55555555-5555-5555-5555-555555555501"
            user_ids = [str(item.get("userId")) for item in items if item.get("userId")]
            if accept_user not in user_ids:
                report.failures.append(
                    f"{spec.operation_id}/owner_control transition invite_accepted missing accepted member"
                )
        except RuntimeError as error:
            report.failures.append(f"{spec.operation_id}/owner_control transition invite_accepted: {error}")
        return transition
    if transition == "ask_stream_started":
        if spec.operation_id != "askStream" or response.status != 200:
            report.failures.append(
                f"{spec.operation_id}/owner_control transition ask_stream_started expected live stream start"
            )
            return transition
        try:
            validate_sse_envelope(
                response.body,
                operation_id="askStream",
                headers=response.headers,
            )
        except RuntimeError as error:
            report.failures.append(f"{spec.operation_id}/owner_control transition ask_stream_started sse: {error}")
        return transition
    if transition == "ask_stream_revoked":
        stream_path = f"{API_PREFIX}/ask/stream"
        stream = http_request(
            method="POST",
            url=api_base.rstrip("/") + stream_path,
            token=credentials.beta_alpha_access_token,
            body={
                "question": f"phase1c-revoke-{seed.challenge[:8]}",
                "collectionIds": [seed.alpha_collection_id],
            },
            path=stream_path,
            accept="text/event-stream",
        )
        if stream.status != 403:
            report.failures.append(
                f"{spec.operation_id}/owner_control transition ask_stream_revoked expected 403 got {stream.status}"
            )
        return transition
    report.failures.append(f"{spec.operation_id}/owner_control transition {transition} missing follow-up")
    return transition


def _mint_download_capability(
    http_request: HttpRequestFn,
    *,
    api_base: str,
    seed: Any,
    credentials: Any,
    token: str,
) -> str:
    path = (
        f"{API_PREFIX}/documents/{seed.beta_document_id}/versions/{seed.beta_version_id}/download-capability"
    )
    response = http_request(
        method="POST",
        url=api_base.rstrip("/") + path,
        token=token,
        body={"purpose": "markdown"},
        path=path,
    )
    if response.status // 100 != 2:
        raise RuntimeError(f"download capability mint failed with {response.status}")
    payload = json.loads(response.body)
    capability = payload.get("capability")
    if not isinstance(capability, str) or not capability.strip():
        raise RuntimeError("download capability mint response missing capability")
    return capability.strip()


def _mint_fake_server_request_id() -> str:
    return str(uuid.uuid4())


def execute_http_denial_suite(
    *,
    seed: Any,
    credentials: Any,
    http_request: HttpRequestFn,
    api_base: str,
    git_sha_full: str,
    manifest_path: Path = DEFAULT_MANIFEST,
    guard_path: Path = DEFAULT_GUARD_INVENTORY,
) -> DenialExecutionReport:
    validate_revision_binding(
        source_revision=seed.source_revision,
        manifest_sha256=seed.manifest_sha256,
        git_sha_full=git_sha_full,
        manifest_path=manifest_path,
    )
    mapping = build_http_sse_denial_mapping(manifest_path=manifest_path, guard_path=guard_path)
    specs = build_denial_request_specs(mapping, seed=seed, credentials=credentials)
    report = DenialExecutionReport(
        schema_version=1,
        git_sha_full=git_sha_full,
        manifest_sha256=seed.manifest_sha256,
        challenge=seed.challenge,
        executable_http_sse_count=len(mapping),
    )
    forbidden = {seed.marker_alpha, seed.marker_beta}
    forbidden.discard("")
    redeemed_download_capabilities: set[str] = set()
    for spec in specs:
        if spec.operation_id == "redeemDownload" and (
            spec.mint_fresh_download_capability
            or spec.scenario in {"owner_control", "preview_download_redeem"}
        ):
            mint_token = spec.token or credentials.alpha_beta_access_token
            capability = _mint_download_capability(
                http_request,
                api_base=api_base,
                seed=seed,
                credentials=credentials,
                token=str(mint_token),
            )
            spec.path = f"{API_PREFIX}/downloads/{capability}"
        url = api_base.rstrip("/") + spec.path
        response = http_request(
            method=spec.method,
            url=url,
            token=spec.token,
            body=spec.body,
            path=spec.path,
            content_type=spec.content_type,
            multipart_body=spec.multipart_body,
            supplied_request_id=spec.supplied_request_id,
            accept=spec.accept,
        )
        scan_body = response.body
        if spec.scenario == "authenticated_foreign_isolation":
            leaked = scan_marker_leakage(scan_body, forbidden_markers={seed.marker_beta})
        elif spec.scenario in {
            "foreign",
            "membership_missing",
            "invalid_capability",
            "invalid_invite_token",
            "invite_already_member",
            "citation_repeat",
            "citation_expired",
            "citation_mismatch",
            "duplicate_name_foreign_oracle",
            "stale_token",
            "stale_token_after_downgrade",
            "stale_token_after_remove",
            "unauthenticated",
            "preview_download_preview_foreign",
        }:
            leaked = scan_marker_leakage(scan_body, forbidden_markers=forbidden)
        else:
            leaked = []
        if spec.operation_id == "redeemDownload" and response.status // 100 == 2:
            cap_token = spec.path.rsplit("/", 1)[-1]
            if cap_token in redeemed_download_capabilities:
                leaked = list(set(leaked) | {f"download-replay:{cap_token[:8]}"})
            redeemed_download_capabilities.add(cap_token)
        if response.status not in spec.expected_statuses:
            report.failures.append(
                f"{spec.operation_id}/{spec.scenario} expected {sorted(spec.expected_statuses)} got {response.status}"
            )
        try:
            server_request_id = validate_server_request_id(body=response.body, headers=response.headers)
        except RuntimeError as error:
            report.failures.append(f"{spec.operation_id}/{spec.scenario} request-id: {error}")
            server_request_id = extract_server_request_id(response.body, response.headers)
        if leaked:
            report.leakage_count += len(leaked)
            report.failures.append(
                f"{spec.operation_id}/{spec.scenario} leaked markers: {', '.join(leaked)}"
            )
        transition: str | None = None
        if spec.scenario == "owner_control" and response.status // 100 == 2:
            transition = spec.owner_transition
            if spec.operation_id in {"askStream", "jobEvents"}:
                try:
                    server_request_id = validate_server_request_id(body=response.body, headers=response.headers)
                    validate_sse_envelope(
                        response.body,
                        operation_id=spec.operation_id,
                        headers=response.headers,
                        expected_request_id=server_request_id,
                    )
                except RuntimeError as error:
                    report.failures.append(f"{spec.operation_id}/owner_control sse: {error}")
            elif spec.success_schema_keys and spec.operation_id not in {"askStream", "jobEvents"}:
                try:
                    if spec.operation_id in HTTP_OWNER_SUCCESS_SCHEMA and spec.method == "GET":
                        validate_owner_read_response(
                            spec.operation_id,
                            response.body,
                            seed=seed,
                            credentials=credentials,
                            path=spec.path,
                        )
                    else:
                        _validate_success_schema(response.body, spec.success_schema_keys)
                except RuntimeError as error:
                    report.failures.append(f"{spec.operation_id}/owner_control schema: {error}")
        if spec.owner_transition and response.status in spec.expected_statuses:
            transition = _validate_owner_transition(
                spec,
                response=response,
                seed=seed,
                credentials=credentials,
                http_request=http_request,
                api_base=api_base,
                transition=spec.owner_transition,
                report=report,
            )
        report.observations.append(
            DenialObservation(
                operation_id=spec.operation_id,
                row_id=spec.row_id,
                scenario=spec.scenario,
                expected_statuses=sorted(spec.expected_statuses),
                actual_status=response.status,
                body_sha256=sha256_text(response.body),
                request_id=server_request_id,
                leaked_markers=leaked,
                owner_transition=transition,
                coverage_limited=spec.coverage_limited,
            )
        )
    try:
        validate_denial_observation_matrix(report.observations)
    except RuntimeError as error:
        report.failures.append(str(error))
    return report


def parse_denial_execution_report(report: dict[str, Any]) -> dict[str, Any]:
    if report.get("schemaVersion") != 1:
        raise RuntimeError("denial report schemaVersion mismatch")
    if "leakageCount" not in report:
        raise RuntimeError("denial report missing leakageCount")
    leakage = report["leakageCount"]
    if isinstance(leakage, bool) or not isinstance(leakage, int):
        raise TypeError("denial leakageCount must be int")
    if report.get("failures"):
        raise RuntimeError("denial runner reported failures")
    redaction = report.get("redactionScan")
    if not isinstance(redaction, dict) or redaction.get("passed") is not True:
        raise RuntimeError("denial report redactionScan failed")
    for field in ("manifestSha256", "gitShaFull"):
        value = report.get(field)
        if not isinstance(value, str) or not value.strip():
            raise RuntimeError(f"denial report missing {field}")
    return {"cross_tenant_leakage_count": leakage}
