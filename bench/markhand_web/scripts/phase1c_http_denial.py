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
DISPOSABLE_MEMBER_OPS = frozenset({"deleteMember", "patchMember"})
DISPOSABLE_CONFLICT_OPS = frozenset({"triageConflict"})

SECONDARY_ROW_VARIANTS: dict[str, str] = {
    "denial-resolveCitation-citation": "citation_matrix",
    "denial-previewDocument-citation": "citation_preview",
    "denial-task13-indexed-fts-ask": "indexed_fts",
    "denial-task13-duplicate-names": "duplicate_names",
    "denial-task13-stale-tokens": "stale_tokens",
    "denial-task13-preview-download-sse": "preview_download_sse",
    "denial-task13-in-flight-ask-revoke": "in_flight_revoke",
}


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
                }
                for item in self.observations
            ],
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
        disposable_member = _credential_string(
            credentials, "beta_denial_disposable_member_user_id", field="betaDenialDisposableMemberUserId"
        )
        if params.get("userId") == disposable_member:
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
    if operation_id in DISPOSABLE_MEMBER_OPS:
        params["userId"] = _credential_string(
            credentials, "beta_denial_disposable_member_user_id", field="betaDenialDisposableMemberUserId"
        )
    if operation_id in DISPOSABLE_CONFLICT_OPS:
        params["conflictId"] = _credential_string(
            credentials, "beta_denial_disposable_conflict_id", field="betaDenialDisposableConflictId"
        )
    return params


def _variant_query_suffix(row_id: str, challenge: str) -> str:
    variant = SECONDARY_ROW_VARIANTS.get(row_id)
    if not variant:
        return challenge[:8]
    return f"{variant}-{challenge[:8]}"


def _body_for_operation(
    operation_id: str,
    seed: Any,
    *,
    credentials: Any,
    foreign: bool,
    row_id: str = "",
) -> dict[str, Any] | None:
    collection_id = seed.beta_collection_id if foreign else seed.alpha_collection_id
    document_id = seed.beta_document_id if foreign else seed.alpha_document_id
    org_id = seed.org_beta_id if foreign else seed.org_alpha_id
    suffix = _variant_query_suffix(row_id, seed.challenge)
    if operation_id in {"search", "ask"}:
        return {"query": f"phase1c-denial-{suffix}", "collectionIds": [collection_id]}
    if operation_id == "askStream":
        return {"query": f"phase1c-denial-stream-{suffix}", "collectionIds": [collection_id]}
    if operation_id == "switchOrg":
        return {"orgId": org_id}
    if operation_id == "createOrg":
        return {"slug": f"denial-probe-{seed.challenge[:8]}", "name": "Denial Probe Org"}
    if operation_id == "createCollection":
        return {
            "name": f"denial-{seed.challenge[:8]}",
            "slug": f"denial-{seed.challenge[:8]}",
            "visibility": "org",
        }
    if operation_id == "createProject":
        return {"name": f"denial-project-{seed.challenge[:8]}"}
    if operation_id == "createUpload":
        return {"collectionId": collection_id, "filename": "denial.txt", "contentType": "text/plain"}
    if operation_id == "createMemberInvite":
        return {"email": f"invite-{seed.challenge[:8]}@example.com", "role": "viewer"}
    if operation_id == "acceptMemberInvite":
        return {"token": _credential_string(credentials, "beta_denial_accept_invite_token", field="betaDenialAcceptInviteToken")}
    if operation_id == "resolveCitation":
        return _resolve_citation_body(seed, credentials=credentials, foreign=foreign)
    if operation_id == "appendChatTurn":
        return {"role": "user", "content": f"phase1c-{seed.challenge[:8]}"}
    if operation_id == "createChatSession":
        return {"title": f"phase1c-{seed.challenge[:8]}"}
    if operation_id == "updateChatSession":
        return {"title": f"phase1c-updated-{seed.challenge[:8]}"}
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
        return {"status": "resolved", "resolutionNote": f"phase1c-{seed.challenge[:8]}"}
    if operation_id == "updateCollection":
        return {"name": f"denial-updated-{seed.challenge[:8]}"}
    if operation_id == "updateProject":
        return {"name": f"denial-project-updated-{seed.challenge[:8]}"}
    if operation_id in {"revokeMemberInvite"}:
        return {}
    return None


def _uses_foreign_scope(operation_id: str, path_template: str) -> frozenset[str]:
    reasons: set[str] = set()
    if operation_id == "createUpload":
        reasons.add("createUpload")
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
            "{capability}",
        )
    ):
        reasons.add("path_template")
    if operation_id in {
        "search",
        "ask",
        "askStream",
        "assignCollectionProject",
        "resolveCitation",
        "switchOrg",
        "acceptMemberInvite",
    }:
        reasons.add(operation_id)
    return frozenset(reasons)


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
        return {"query": f"phase1c-owner-{seed.challenge[:8]}", "collectionIds": [seed.beta_collection_id]}
    if operation_id == "askStream":
        return {"query": f"phase1c-owner-{seed.challenge[:8]}", "collectionIds": [seed.beta_collection_id]}
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
        return {"role": "user", "content": f"phase1c-owner-{seed.challenge[:8]}"}
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
    mapping: dict[str, frozenset[str]] = {
        "getCollection": frozenset({"id"}),
        "getDocument": frozenset({"id"}),
        "getJob": frozenset({"id"}),
        "getDocumentVersion": frozenset({"id", "versionId"}),
        "getConflict": frozenset({"id"}),
        "getChatSession": frozenset({"id"}),
        "getProject": frozenset({"id"}),
        "getOrg": frozenset({"id"}),
        "listConflicts": frozenset({"items"}),
        "listDocuments": frozenset({"items"}),
        "listCollections": frozenset({"items"}),
        "listProjects": frozenset({"items"}),
        "listMembers": frozenset({"items"}),
        "listMemberInvites": frozenset({"items"}),
        "listChatSessions": frozenset({"items"}),
        "listDocumentVersions": frozenset({"items"}),
        "listOrgs": frozenset({"items"}),
        "listAudit": frozenset({"items"}),
        "getUsage": frozenset({"documents"}),
        "getGraph": frozenset({"nodes"}),
        "getConflictEvidence": frozenset({"items"}),
        "authMe": frozenset({"userId", "sessionId"}),
        "resolveCitation": frozenset({"citation", "requestId"}),
        "previewDocument": frozenset({"documentId"}),
        "diffDocumentVersions": frozenset({"fromVersionId"}),
    }
    return mapping.get(operation_id, frozenset({"id"}))


def build_owner_control_spec(
    entry: DenialMappingEntry,
    *,
    seed: Any,
    credentials: Any,
) -> OwnerControlSpec | None:
    params = _owner_params(seed, credentials=credentials, operation_id=entry.operation_id)
    path = _substitute_path(entry.path_template, params)
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
            token=None,
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


def build_denial_request_specs(
    mapping: list[DenialMappingEntry],
    *,
    seed: Any,
    credentials: Any,
) -> list[DenialRequestSpec]:
    specs: list[DenialRequestSpec] = []
    params = _foreign_params(seed, credentials=credentials)
    for entry in mapping:
        path = _substitute_path(entry.path_template, params)
        body = _body_for_operation(
            entry.operation_id,
            seed,
            credentials=credentials,
            foreign=True,
            row_id=entry.row_id,
        )
        request_id = f"phase1c-{entry.row_id}-{secrets.token_hex(4)}"
        if _uses_foreign_scope(entry.operation_id, entry.path_template):
            owner = build_owner_control_spec(entry, seed=seed, credentials=credentials)
            if owner is not None:
                specs.append(
                    DenialRequestSpec(
                        row_id=entry.row_id,
                        operation_id=entry.operation_id,
                        scenario="owner_control",
                        method=owner.method,
                        path=owner.path,
                        token=owner.token if owner.token is not None else credentials.alpha_beta_access_token,
                        body=owner.body,
                        expected_statuses=owner.expected_statuses,
                        content_type=owner.content_type,
                        multipart_body=owner.multipart_body,
                        supplied_request_id=f"{request_id}-owner",
                        success_schema_keys=owner.success_schema_keys,
                        accept=owner.accept,
                    )
                )
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
            specs.append(
                DenialRequestSpec(
                    row_id=entry.row_id,
                    operation_id=entry.operation_id,
                    scenario="foreign",
                    method=entry.method,
                    path=path,
                    token=credentials.alpha_access_token,
                    body=body,
                    expected_statuses=frozenset({403, 404}),
                    content_type=content_type,
                    multipart_body=upload_multipart,
                    supplied_request_id=f"{request_id}-foreign",
                )
            )
    return specs


def scan_marker_leakage(body: str, *, forbidden_markers: set[str]) -> list[str]:
    leaks: list[str] = []
    for marker in sorted(forbidden_markers):
        if marker and marker in body:
            leaks.append(marker)
    return leaks


def validate_denial_observation_matrix(observations: list[DenialObservation]) -> None:
    foreign_rows = {item.row_id for item in observations if item.scenario == "foreign"}
    if not foreign_rows:
        return
    by_row: dict[str, list[DenialObservation]] = {}
    for item in observations:
        by_row.setdefault(item.row_id, []).append(item)
    for row_id, rows in by_row.items():
        foreign = [row for row in rows if row.scenario == "foreign"]
        owner = [row for row in rows if row.scenario == "owner_control"]
        if foreign and owner:
            if all(row.actual_status == 403 for row in foreign) and all(row.actual_status == 403 for row in owner):
                operation_id = foreign[0].operation_id
                raise RuntimeError(f"all-403 shim detected for {operation_id}/{row_id}")
    owner_rows = {item.row_id for item in observations if item.scenario == "owner_control"}
    missing = foreign_rows - owner_rows
    if missing:
        raise RuntimeError(f"foreign rows missing owner_control: {sorted(missing)}")


def _validate_success_schema(body: str, required_keys: frozenset[str]) -> None:
    payload = json.loads(body)
    if not isinstance(payload, dict):
        raise RuntimeError("owner control response must be object")
    for key in required_keys:
        if key not in payload:
            raise RuntimeError(f"owner control response missing {key}")


def _validate_sse_envelope(body: str, *, operation_id: str, headers: dict[str, str]) -> None:
    content_type = ""
    for key, value in headers.items():
        if key.lower() == "content-type":
            content_type = value.lower()
            break
    if "text/event-stream" not in content_type:
        raise RuntimeError(f"{operation_id} SSE response missing text/event-stream content-type")
    if not body.strip():
        raise RuntimeError(f"{operation_id} SSE response empty")
    if "event:" not in body and "data:" not in body:
        raise RuntimeError(f"{operation_id} SSE response missing event envelope")


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
    for spec in specs:
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
        if spec.scenario in {"foreign", "unauthenticated"}:
            leaked = scan_marker_leakage(scan_body, forbidden_markers=forbidden)
        else:
            leaked = []
        if response.status not in spec.expected_statuses:
            report.failures.append(
                f"{spec.operation_id}/{spec.scenario} expected {sorted(spec.expected_statuses)} got {response.status}"
            )
        if spec.scenario == "owner_control" and response.status // 100 == 2:
            if spec.success_schema_keys:
                try:
                    _validate_success_schema(response.body, spec.success_schema_keys)
                except RuntimeError as error:
                    report.failures.append(f"{spec.operation_id}/owner_control schema: {error}")
            if spec.operation_id in {"askStream", "jobEvents"}:
                try:
                    _validate_sse_envelope(
                        response.body,
                        operation_id=spec.operation_id,
                        headers=response.headers,
                    )
                except RuntimeError as error:
                    report.failures.append(f"{spec.operation_id}/owner_control sse: {error}")
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
