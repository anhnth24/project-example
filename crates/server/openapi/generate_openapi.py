#!/usr/bin/env python3
"""Generate crates/server/openapi/openapi.yaml (run from repo root or this dir)."""

from __future__ import annotations

from pathlib import Path
import copy

import yaml

ROOT = Path(__file__).resolve().parent


def op(
    operation_id: str,
    tag: str,
    summary: str,
    *,
    security: bool = False,
    parameters: list | None = None,
    request_body: dict | None = None,
    responses: dict | None = None,
) -> dict:
    body: dict = {
        "operationId": operation_id,
        "tags": [tag],
        "summary": summary,
        "responses": responses or {},
    }
    if security:
        body["security"] = [{"bearerAuth": []}]
    if parameters:
        body["parameters"] = parameters
    if request_body is not None:
        body["requestBody"] = request_body
    return body


def json_schema(ref: str) -> dict:
    return {"application/json": {"schema": {"$ref": ref}}}


def ok(schema_ref: str, description: str = "OK") -> dict:
    return {
        "description": description,
        "headers": {"X-Request-Id": {"$ref": "#/components/headers/RequestId"}},
        "content": json_schema(schema_ref),
    }


def err(code: str = "ApiError") -> dict:
    return {"$ref": f"#/components/responses/{code}"}


P = {
    "limit": {"$ref": "#/components/parameters/Limit"},
    "cursor": {"$ref": "#/components/parameters/Cursor"},
    "collectionId": {"$ref": "#/components/parameters/CollectionId"},
    "documentId": {"$ref": "#/components/parameters/DocumentId"},
    "versionId": {"$ref": "#/components/parameters/VersionId"},
    "conflictId": {"$ref": "#/components/parameters/ConflictId"},
    "idem": {"$ref": "#/components/parameters/IdempotencyKey"},
}


def health_get_head(operation_id: str, summary: str, responses: dict) -> dict:
    return {
        "get": op(operation_id, "Health", summary, responses=responses),
        "head": op(f"{operation_id}Head", "Health", f"{summary} (HEAD)", responses=responses),
    }


def main() -> None:
    api_paths = {
        "/health/live": health_get_head(
            "healthLive",
            "Process liveness",
            {"200": ok("#/components/schemas/Health", "Alive")},
        ),
        "/health/ready": health_get_head(
            "healthReady",
            "Readiness (deps/signature/reconciliation)",
            {
                "200": ok("#/components/schemas/Health", "Ready"),
                "503": err(),
                "429": err("RateLimited"),
            },
        ),
        "/health/startup": health_get_head(
            "healthStartup",
            "One-way startup completion",
            {
                "200": ok("#/components/schemas/Startup", "Startup completed"),
                "503": {
                    "description": "Still starting",
                    "content": json_schema("#/components/schemas/Startup"),
                },
            },
        ),
        "/auth/login": {
            "post": op(
                "authLogin",
                "Auth",
                "Password login",
                request_body={
                    "required": True,
                    "content": json_schema("#/components/schemas/LoginRequest"),
                },
                responses={
                    "200": ok("#/components/schemas/TokenResponse", "Tokens"),
                    "400": err(),
                    "401": err(),
                    "429": err("RateLimited"),
                },
            )
        },
        "/auth/refresh": {
            "post": op(
                "authRefresh",
                "Auth",
                "Refresh session",
                request_body={
                    "required": True,
                    "content": json_schema("#/components/schemas/RefreshRequest"),
                },
                responses={
                    "200": ok("#/components/schemas/TokenResponse", "Rotated tokens"),
                    "401": err(),
                    "429": err("RateLimited"),
                },
            )
        },
        "/auth/logout": {
            "post": op(
                "authLogout",
                "Auth",
                "Logout / revoke refresh family",
                request_body={
                    "required": True,
                    "content": json_schema("#/components/schemas/LogoutRequest"),
                },
                responses={"204": {"description": "Logged out"}, "401": err()},
            )
        },
        "/auth/me": {
            "get": op(
                "authMe",
                "Auth",
                "Current principal",
                security=True,
                responses={
                    "200": ok("#/components/schemas/MeResponse", "Current principal"),
                    "401": err(),
                },
            )
        },
        "/collections": {
            "get": op(
                "listCollections",
                "Collections",
                "List collections",
                security=True,
                parameters=[P["limit"], P["cursor"]],
                responses={
                    "200": ok("#/components/schemas/CollectionList", "Collection page"),
                    "401": err(),
                    "403": err(),
                },
            ),
            "post": op(
                "createCollection",
                "Collections",
                "Create collection",
                security=True,
                request_body={
                    "required": True,
                    "content": json_schema("#/components/schemas/CollectionCreate"),
                },
                responses={
                    "201": ok("#/components/schemas/Collection", "Created"),
                    "400": err(),
                    "401": err(),
                    "403": err(),
                    "429": err("RateLimited"),
                },
            ),
        },
        "/collections/{collectionId}": {
            "get": op(
                "getCollection",
                "Collections",
                "Get collection",
                security=True,
                parameters=[P["collectionId"]],
                responses={
                    "200": ok("#/components/schemas/Collection", "Collection"),
                    "401": err(),
                    "404": err(),
                },
            ),
            "patch": op(
                "updateCollection",
                "Collections",
                "Update collection",
                security=True,
                parameters=[P["collectionId"]],
                request_body={
                    "required": True,
                    "content": json_schema("#/components/schemas/CollectionUpdate"),
                },
                responses={
                    "200": ok("#/components/schemas/Collection", "Updated"),
                    "400": err(),
                    "401": err(),
                    "404": err(),
                },
            ),
        },
        "/documents": {
            "get": op(
                "listDocuments",
                "Documents",
                "List documents",
                security=True,
                parameters=[P["limit"], P["cursor"]],
                responses={
                    "200": ok("#/components/schemas/DocumentList", "Document page"),
                    "401": err(),
                },
            )
        },
        "/documents/{documentId}": {
            "get": op(
                "getDocument",
                "Documents",
                "Get document",
                security=True,
                parameters=[P["documentId"]],
                responses={
                    "200": ok("#/components/schemas/Document", "Document"),
                    "404": err(),
                },
            ),
            "delete": op(
                "deleteDocument",
                "Documents",
                "Tombstone document",
                security=True,
                parameters=[P["documentId"]],
                responses={
                    "200": ok("#/components/schemas/Document", "Tombstoned"),
                    "404": err(),
                },
            ),
        },
        "/documents/{documentId}/reindex": {
            "post": op(
                "reindexDocument",
                "Documents",
                "Enqueue reindex",
                security=True,
                parameters=[P["documentId"], P["idem"]],
                responses={
                    "200": ok("#/components/schemas/ReindexResponse", "Reindex enqueued"),
                    "409": err(),
                },
            )
        },
        "/documents/{documentId}/versions": {
            "get": op(
                "listDocumentVersions",
                "Documents",
                "List versions",
                security=True,
                parameters=[P["documentId"], P["limit"], P["cursor"]],
                responses={
                    "200": ok("#/components/schemas/DocumentVersionList", "Version page")
                },
            )
        },
        "/documents/{documentId}/versions/{versionId}": {
            "get": op(
                "getDocumentVersion",
                "Documents",
                "Get version metadata (no object keys)",
                security=True,
                parameters=[P["documentId"], P["versionId"]],
                responses={
                    "200": ok("#/components/schemas/DocumentVersion", "Version metadata"),
                    "404": err(),
                },
            )
        },
        "/documents/{documentId}/versions/{versionId}/publish": {
            "post": op(
                "publishDocumentVersion",
                "Documents",
                "Publish version",
                security=True,
                parameters=[P["documentId"], P["versionId"]],
                responses={
                    "200": ok("#/components/schemas/DocumentVersion", "Published")
                },
            )
        },
        "/documents/{documentId}/versions/{leftVersionId}/diff/{rightVersionId}": {
            "get": op(
                "diffDocumentVersions",
                "Documents",
                "Metadata diff between versions",
                security=True,
                parameters=[
                    P["documentId"],
                    {
                        "name": "leftVersionId",
                        "in": "path",
                        "required": True,
                        "schema": {"type": "string", "format": "uuid"},
                    },
                    {
                        "name": "rightVersionId",
                        "in": "path",
                        "required": True,
                        "schema": {"type": "string", "format": "uuid"},
                    },
                ],
                responses={"200": ok("#/components/schemas/VersionDiff", "Metadata diff")},
            )
        },
        "/documents/{documentId}/versions/{versionId}/preview": {
            "get": op(
                "previewDocumentVersion",
                "Documents",
                "Authorized markdown preview",
                security=True,
                parameters=[P["documentId"], P["versionId"]],
                responses={
                    "200": ok("#/components/schemas/PreviewResponse", "Preview"),
                    "403": err(),
                    "404": err(),
                },
            )
        },
        "/documents/{documentId}/versions/{versionId}/download-capabilities": {
            "post": op(
                "mintDownloadCapability",
                "Documents",
                "Mint short-lived download capability",
                security=True,
                parameters=[P["documentId"], P["versionId"]],
                request_body={
                    "required": True,
                    "content": json_schema("#/components/schemas/DownloadCapabilityRequest"),
                },
                responses={
                    "200": ok(
                        "#/components/schemas/DownloadCapabilityResponse",
                        "Capability token",
                    ),
                    "400": err(),
                    "401": err(),
                    "403": err(),
                    "429": err("RateLimited"),
                },
            )
        },
        "/download-capabilities/redeem": {
            "post": op(
                "redeemDownloadCapability",
                "Documents",
                "Redeem download capability (binary media body)",
                security=True,
                request_body={
                    "required": True,
                    "content": json_schema("#/components/schemas/DownloadRedeemRequest"),
                },
                responses={
                    "200": {
                        "description": "Authorized binary download body",
                        "headers": {
                            "X-Request-Id": {"$ref": "#/components/headers/RequestId"},
                            "X-Content-Sha256": {
                                "schema": {"type": "string"},
                                "description": "Content digest of the redeemed artifact",
                            },
                            "Content-Disposition": {
                                "schema": {"type": "string"},
                            },
                        },
                        "content": {
                            "application/octet-stream": {
                                "schema": {"type": "string", "format": "binary"}
                            }
                        },
                    },
                    "403": err(),
                    "429": err("RateLimited"),
                },
            )
        },
        "/conflicts": {
            "get": op(
                "listConflicts",
                "Conflicts",
                "List conflicts",
                security=True,
                parameters=[P["limit"], P["cursor"]],
                responses={"200": ok("#/components/schemas/ConflictList", "Conflict page")},
            )
        },
        "/conflicts/{conflictId}": {
            "get": op(
                "getConflict",
                "Conflicts",
                "Get conflict",
                security=True,
                parameters=[P["conflictId"]],
                responses={
                    "200": ok("#/components/schemas/Conflict", "Conflict"),
                    "404": err(),
                },
            )
        },
        "/conflicts/{conflictId}/triage": {
            "post": op(
                "triageConflict",
                "Conflicts",
                "Triage conflict",
                security=True,
                parameters=[P["conflictId"]],
                request_body={
                    "required": True,
                    "content": json_schema("#/components/schemas/ConflictTriageRequest"),
                },
                responses={"200": ok("#/components/schemas/Conflict", "Triaged")},
            )
        },
        "/conflicts/{conflictId}/evidence": {
            "get": op(
                "listConflictEvidence",
                "Conflicts",
                "List conflict evidence",
                security=True,
                parameters=[P["conflictId"], P["limit"], P["cursor"]],
                responses={
                    "200": ok("#/components/schemas/ConflictEvidenceList", "Evidence page")
                },
            )
        },
        "/citations/resolve": {
            "post": op(
                "resolveCitations",
                "Citations",
                "Resolve citation locators",
                security=True,
                request_body={
                    "required": True,
                    "content": json_schema("#/components/schemas/CitationResolveRequest"),
                },
                responses={
                    "200": ok(
                        "#/components/schemas/CitationResolveResponse",
                        "Resolved citations",
                    )
                },
            )
        },
        "/jobs": {
            "get": op(
                "listJobs",
                "Jobs",
                "List jobs",
                security=True,
                parameters=[P["limit"], P["cursor"]],
                responses={"200": ok("#/components/schemas/JobList", "Job page")},
            )
        },
        "/jobs/{jobId}": {
            "get": op(
                "getJob",
                "Jobs",
                "Get job",
                security=True,
                parameters=[
                    {
                        "name": "jobId",
                        "in": "path",
                        "required": True,
                        "schema": {"type": "string", "format": "uuid"},
                    }
                ],
                responses={
                    "200": ok("#/components/schemas/Job", "Job"),
                    "404": err(),
                },
            )
        },
        "/uploads": {
            "post": op(
                "createUpload",
                "Uploads",
                "Streaming quarantine upload",
                security=True,
                parameters=[P["idem"]],
                request_body={
                    "required": True,
                    "content": {
                        "multipart/form-data": {
                            "schema": {
                                "type": "object",
                                "required": ["file"],
                                "properties": {
                                    "file": {"type": "string", "format": "binary"},
                                    "collectionId": {
                                        "type": "string",
                                        "format": "uuid",
                                    },
                                },
                            }
                        }
                    },
                },
                responses={
                    "200": ok(
                        "#/components/schemas/UploadResponse",
                        "Upload accepted (opaque objectId only)",
                    ),
                    "400": err(),
                    "413": err(),
                    "429": err("RateLimited"),
                },
            )
        },
        "/search": {
            "post": op(
                "search",
                "Search",
                "Tenant-scoped hybrid search",
                security=True,
                request_body={
                    "required": True,
                    "content": json_schema("#/components/schemas/SearchRequest"),
                },
                responses={
                    "200": ok("#/components/schemas/SearchResponse", "Authorized hits"),
                    "400": err(),
                    "401": err(),
                    "429": err("RateLimited"),
                },
            )
        },
        "/ask": {
            "post": op(
                "ask",
                "Ask",
                "Grounded Q&A",
                security=True,
                request_body={
                    "required": True,
                    "content": json_schema("#/components/schemas/AskRequest"),
                },
                responses={
                    "200": ok("#/components/schemas/AskResponse", "Grounded answer"),
                    "400": err(),
                    "429": err("RateLimited"),
                },
            )
        },
        "/ask/stream": {
            "post": op(
                "askStream",
                "Ask",
                "Closed-snapshot SSE Q&A delivery",
                security=True,
                request_body={
                    "required": True,
                    "content": json_schema("#/components/schemas/AskRequest"),
                },
                responses={
                    "200": {
                        "description": "Closed-snapshot SSE delivery",
                        "content": {
                            "text/event-stream": {
                                "schema": {"$ref": "#/components/schemas/SseEnvelope"},
                                "examples": {
                                    "frames": {
                                        "summary": "Repeated canonical SSE frames",
                                        "value": "id: 42\nevent: metadata\ndata: {\"version\":1,\"sequence\":42,\"event\":\"metadata\",\"requestId\":\"a1010000-0000-4000-8000-000000000002\",\"data\":{}}\n\nid: 43\nevent: token\ndata: {\"version\":1,\"sequence\":43,\"event\":\"token\",\"requestId\":\"a1010000-0000-4000-8000-000000000002\",\"data\":{\"text\":\"hi\"}}\n\n",
                                    }
                                },
                            }
                        },
                    },
                    "400": err(),
                    "401": err(),
                    "429": err("RateLimited"),
                },
            )
        },
        "/events/{requestId}": {
            "get": op(
                "resumeEvents",
                "Ask",
                "Resume closed SSE snapshot",
                security=True,
                parameters=[
                    {
                        "name": "requestId",
                        "in": "path",
                        "required": True,
                        "schema": {"type": "string", "format": "uuid"},
                    },
                    {
                        "name": "Last-Event-ID",
                        "in": "header",
                        "required": False,
                        "schema": {"type": "string"},
                    },
                ],
                responses={
                    "200": {
                        "description": "Resumable closed SSE snapshot",
                        "content": {
                            "text/event-stream": {
                                "schema": {"$ref": "#/components/schemas/SseEnvelope"},
                                "examples": {
                                    "frames": {
                                        "summary": "Repeated canonical SSE frames",
                                        "value": "id: 42\nevent: metadata\ndata: {\"version\":1,\"sequence\":42,\"event\":\"metadata\",\"requestId\":\"a1010000-0000-4000-8000-000000000002\",\"data\":{}}\n\nid: 43\nevent: token\ndata: {\"version\":1,\"sequence\":43,\"event\":\"token\",\"requestId\":\"a1010000-0000-4000-8000-000000000002\",\"data\":{\"text\":\"hi\"}}\n\n",
                                    }
                                },
                            }
                        },
                    },
                    "400": err(),
                    "404": err(),
                    "410": err(),
                },
            )
        },
    }

    root_health = {
        "/live": health_get_head(
            "rootLive",
            "Process liveness (root)",
            {"200": ok("#/components/schemas/Health", "Alive")},
        ),
        "/ready": health_get_head(
            "rootReady",
            "Readiness (root)",
            {
                "200": ok("#/components/schemas/Health", "Ready"),
                "503": err(),
                "429": err("RateLimited"),
            },
        ),
        "/startup": health_get_head(
            "rootStartup",
            "One-way startup completion (root)",
            {
                "200": ok("#/components/schemas/Startup", "Startup completed"),
                "503": {
                    "description": "Still starting",
                    "content": json_schema("#/components/schemas/Startup"),
                },
            },
        ),
    }
    paths = {**root_health, **{f"/api/v1{path}": body for path, body in api_paths.items()}}

    doc = {
        "openapi": "3.1.0",
        "info": {
            "title": "Markhand Web API",
            "version": "0.1.0",
            "description": (
                "Phase 1B POC `/api/v1` contract (R02/R04/R05/R06). "
                "Does not expose secrets or internal object keys."
            ),
        },
        "servers": [{"url": "/"}],
        "tags": [
            {"name": "Health"},
            {"name": "Auth"},
            {"name": "Collections"},
            {"name": "Documents"},
            {"name": "Conflicts"},
            {"name": "Citations"},
            {"name": "Jobs"},
            {"name": "Uploads"},
            {"name": "Search"},
            {"name": "Ask"},
        ],
        "paths": paths,
        "components": {
            "securitySchemes": {
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "bearerFormat": "JWT",
                }
            },
            "headers": {
                "RequestId": {
                    "description": "Validated or server-generated request id",
                    "schema": {"type": "string", "format": "uuid"},
                },
                "RetryAfter": {
                    "description": "Seconds until rate-limit window resets",
                    "schema": {"type": "integer", "minimum": 1},
                },
            },
            "parameters": {
                "Limit": {
                    "name": "limit",
                    "in": "query",
                    "schema": {"type": "integer", "minimum": 1, "maximum": 100},
                },
                "Cursor": {"name": "cursor", "in": "query", "schema": {"type": "string"}},
                "CollectionId": {
                    "name": "collectionId",
                    "in": "path",
                    "required": True,
                    "schema": {"type": "string", "format": "uuid"},
                },
                "DocumentId": {
                    "name": "documentId",
                    "in": "path",
                    "required": True,
                    "schema": {"type": "string", "format": "uuid"},
                },
                "VersionId": {
                    "name": "versionId",
                    "in": "path",
                    "required": True,
                    "schema": {"type": "string", "format": "uuid"},
                },
                "ConflictId": {
                    "name": "conflictId",
                    "in": "path",
                    "required": True,
                    "schema": {"type": "string", "format": "uuid"},
                },
                "IdempotencyKey": {
                    "name": "Idempotency-Key",
                    "in": "header",
                    "required": False,
                    "schema": {"type": "string", "maxLength": 256},
                },
            },
            "responses": {
                "ApiError": {
                    "description": "Canonical API error",
                    "headers": {
                        "X-Request-Id": {"$ref": "#/components/headers/RequestId"}
                    },
                    "content": json_schema("#/components/schemas/ApiError"),
                },
                "RateLimited": {
                    "description": "Rate limit exceeded",
                    "headers": {
                        "Retry-After": {"$ref": "#/components/headers/RetryAfter"},
                        "X-RateLimit-Limit": {
                            "description": "Configured request budget for the active scope",
                            "schema": {"type": "integer", "minimum": 0},
                        },
                        "X-RateLimit-Remaining": {
                            "description": "Remaining requests in the current window",
                            "schema": {"type": "integer", "minimum": 0},
                        },
                        "X-RateLimit-Reset": {
                            "description": "Seconds until the current window resets",
                            "schema": {"type": "integer", "minimum": 1},
                        },
                        "X-Request-Id": {"$ref": "#/components/headers/RequestId"},
                    },
                    "content": json_schema("#/components/schemas/RateLimitedError"),
                },
            },
            "schemas": {
                "Health": {
                    "type": "object",
                    "required": ["status", "requestId"],
                    "properties": {
                        "status": {"type": "string", "enum": ["ok"]},
                        "requestId": {"type": "string", "format": "uuid"},
                    },
                },
                "Startup": {
                    "type": "object",
                    "required": ["status", "completed", "degraded", "requestId"],
                    "properties": {
                        "status": {
                            "type": "string",
                            "enum": ["ok", "degraded", "starting"],
                        },
                        "completed": {"type": "boolean"},
                        "degraded": {"type": "boolean"},
                        "requestId": {"type": "string", "format": "uuid"},
                    },
                },
                "ApiError": {
                    "type": "object",
                    "required": ["code", "message", "requestId"],
                    "properties": {
                        "code": {"type": "string"},
                        "message": {"type": "string"},
                        "requestId": {"type": "string", "format": "uuid"},
                        "details": {},
                    },
                },
                "RateLimitedError": {
                    "type": "object",
                    "required": ["code", "message", "requestId", "details"],
                    "properties": {
                        "code": {"type": "string", "enum": ["rate_limited"]},
                        "message": {"type": "string"},
                        "requestId": {"type": "string", "format": "uuid"},
                        "details": {
                            "type": "object",
                            "required": ["limit", "remaining", "resetSecs", "scope"],
                            "properties": {
                                "limit": {"type": "integer"},
                                "remaining": {"type": "integer"},
                                "resetSecs": {"type": "integer"},
                                "scope": {"type": "string"},
                                "quota": {
                                    "type": "object",
                                    "required": ["limit", "remaining", "resetSecs", "scope"],
                                    "properties": {
                                        "limit": {"type": "integer"},
                                        "remaining": {"type": "integer"},
                                        "resetSecs": {"type": "integer"},
                                        "scope": {"type": "string"},
                                    },
                                },
                            },
                        },
                    },
                },
                "PageInfo": {
                    "type": "object",
                    "required": ["hasMore"],
                    "properties": {
                        "nextCursor": {"type": ["string", "null"]},
                        "hasMore": {"type": "boolean"},
                    },
                },
                "LoginRequest": {
                    "type": "object",
                    "required": ["email", "password"],
                    "properties": {
                        "email": {"type": "string"},
                        "password": {"type": "string", "writeOnly": True},
                    },
                },
                "RefreshRequest": {
                    "type": "object",
                    "required": ["refreshToken"],
                    "properties": {
                        "refreshToken": {"type": "string", "writeOnly": True}
                    },
                },
                "LogoutRequest": {
                    "type": "object",
                    "required": ["refreshToken"],
                    "properties": {
                        "refreshToken": {"type": "string", "writeOnly": True}
                    },
                },
                "TokenResponse": {
                    "type": "object",
                    "required": [
                        "accessToken",
                        "refreshToken",
                        "tokenType",
                        "expiresIn",
                        "orgId",
                        "userId",
                    ],
                    "properties": {
                        "accessToken": {"type": "string"},
                        "refreshToken": {"type": "string"},
                        "tokenType": {"type": "string", "enum": ["Bearer"]},
                        "expiresIn": {"type": "integer"},
                        "orgId": {"type": "string", "format": "uuid"},
                        "userId": {"type": "string", "format": "uuid"},
                    },
                },
                "MeResponse": {
                    "type": "object",
                    "required": [
                        "userId",
                        "orgId",
                        "email",
                        "displayName",
                        "permissions",
                        "allowedCollectionIds",
                        "sessionId",
                    ],
                    "properties": {
                        "userId": {"type": "string", "format": "uuid"},
                        "orgId": {"type": "string", "format": "uuid"},
                        "email": {"type": "string"},
                        "displayName": {"type": "string"},
                        "permissions": {"type": "array", "items": {"type": "string"}},
                        "allowedCollectionIds": {
                            "type": "array",
                            "items": {"type": "string", "format": "uuid"},
                        },
                        "sessionId": {"type": "string"},
                    },
                },
                "Collection": {
                    "type": "object",
                    "required": [
                        "id",
                        "name",
                        "slug",
                        "visibility",
                        "ownerUserId",
                        "createdAt",
                        "updatedAt",
                        "requestId",
                    ],
                    "properties": {
                        "id": {"type": "string", "format": "uuid"},
                        "name": {"type": "string"},
                        "slug": {"type": "string"},
                        "description": {"type": ["string", "null"]},
                        "visibility": {"type": "string"},
                        "ownerUserId": {"type": "string", "format": "uuid"},
                        "createdAt": {"type": "string", "format": "date-time"},
                        "updatedAt": {"type": "string", "format": "date-time"},
                        "requestId": {"type": "string", "format": "uuid"},
                    },
                },
                "CollectionList": {
                    "type": "object",
                    "required": ["items", "pageInfo", "requestId"],
                    "properties": {
                        "items": {
                            "type": "array",
                            "items": {"$ref": "#/components/schemas/Collection"},
                        },
                        "pageInfo": {"$ref": "#/components/schemas/PageInfo"},
                        "requestId": {"type": "string", "format": "uuid"},
                    },
                },
                "CollectionCreate": {
                    "type": "object",
                    "required": ["name", "slug"],
                    "properties": {
                        "name": {"type": "string"},
                        "slug": {"type": "string"},
                        "description": {"type": "string"},
                        "visibility": {"type": "string"},
                    },
                },
                "CollectionUpdate": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "description": {"type": ["string", "null"]},
                        "visibility": {"type": "string"},
                    },
                },
                "Document": {
                    "type": "object",
                    "required": [
                        "id",
                        "collectionId",
                        "title",
                        "state",
                        "createdByUserId",
                        "createdAt",
                        "updatedAt",
                        "requestId",
                    ],
                    "properties": {
                        "id": {"type": "string", "format": "uuid"},
                        "collectionId": {"type": "string", "format": "uuid"},
                        "title": {"type": "string"},
                        "state": {"type": "string"},
                        "currentVersionId": {
                            "type": ["string", "null"],
                            "format": "uuid",
                        },
                        "createdByUserId": {"type": "string", "format": "uuid"},
                        "createdAt": {"type": "string", "format": "date-time"},
                        "updatedAt": {"type": "string", "format": "date-time"},
                        "deletedAt": {"type": ["string", "null"], "format": "date-time"},
                        "requestId": {"type": "string", "format": "uuid"},
                    },
                },
                "DocumentList": {
                    "type": "object",
                    "required": ["items", "pageInfo", "requestId"],
                    "properties": {
                        "items": {
                            "type": "array",
                            "items": {"$ref": "#/components/schemas/Document"},
                        },
                        "pageInfo": {"$ref": "#/components/schemas/PageInfo"},
                        "requestId": {"type": "string", "format": "uuid"},
                    },
                },
                "DocumentVersion": {
                    "type": "object",
                    "description": "Version metadata only — never exposes storage object keys.",
                    "required": [
                        "id",
                        "documentId",
                        "versionNumber",
                        "publicationState",
                        "isCurrent",
                        "contentSha256",
                        "effectiveFrom",
                        "createdByUserId",
                        "createdAt",
                        "requestId",
                    ],
                    "properties": {
                        "id": {"type": "string", "format": "uuid"},
                        "documentId": {"type": "string", "format": "uuid"},
                        "versionNumber": {"type": "integer"},
                        "parentVersionId": {
                            "type": ["string", "null"],
                            "format": "uuid",
                        },
                        "publicationState": {"type": "string"},
                        "isCurrent": {"type": "boolean"},
                        "contentSha256": {"type": "string"},
                        "sourceFilename": {"type": ["string", "null"]},
                        "sourceContentType": {"type": ["string", "null"]},
                        "byteSize": {"type": ["integer", "null"]},
                        "effectiveFrom": {"type": "string", "format": "date-time"},
                        "effectiveTo": {
                            "type": ["string", "null"],
                            "format": "date-time",
                        },
                        "changeSummary": {"type": ["string", "null"]},
                        "createdByUserId": {"type": "string", "format": "uuid"},
                        "createdAt": {"type": "string", "format": "date-time"},
                        "requestId": {"type": "string", "format": "uuid"},
                    },
                },
                "DocumentVersionList": {
                    "type": "object",
                    "required": ["items", "pageInfo", "requestId"],
                    "properties": {
                        "items": {
                            "type": "array",
                            "items": {"$ref": "#/components/schemas/DocumentVersion"},
                        },
                        "pageInfo": {"$ref": "#/components/schemas/PageInfo"},
                        "requestId": {"type": "string", "format": "uuid"},
                    },
                },
                "VersionDiff": {
                    "type": "object",
                    "required": [
                        "documentId",
                        "leftVersionId",
                        "rightVersionId",
                        "leftVersionNumber",
                        "rightVersionNumber",
                        "contentSha256Changed",
                        "publicationStateChanged",
                        "currentFlagChanged",
                        "changeSummaryChanged",
                        "requestId",
                    ],
                    "properties": {
                        "documentId": {"type": "string", "format": "uuid"},
                        "leftVersionId": {"type": "string", "format": "uuid"},
                        "rightVersionId": {"type": "string", "format": "uuid"},
                        "leftVersionNumber": {"type": "integer"},
                        "rightVersionNumber": {"type": "integer"},
                        "contentSha256Changed": {"type": "boolean"},
                        "publicationStateChanged": {"type": "boolean"},
                        "currentFlagChanged": {"type": "boolean"},
                        "changeSummaryChanged": {"type": "boolean"},
                        "requestId": {"type": "string", "format": "uuid"},
                    },
                },
                "ReindexResponse": {
                    "type": "object",
                    "required": [
                        "documentId",
                        "versionId",
                        "jobId",
                        "created",
                        "requestId",
                    ],
                    "properties": {
                        "documentId": {"type": "string", "format": "uuid"},
                        "versionId": {"type": "string", "format": "uuid"},
                        "jobId": {"type": "string", "format": "uuid"},
                        "created": {"type": "boolean"},
                        "requestId": {"type": "string", "format": "uuid"},
                    },
                },
                "PreviewResponse": {
                    "type": "object",
                    "required": ["documentId", "versionId", "markdown", "requestId"],
                    "properties": {
                        "documentId": {"type": "string", "format": "uuid"},
                        "versionId": {"type": "string", "format": "uuid"},
                        "markdown": {"type": "string"},
                        "requestId": {"type": "string", "format": "uuid"},
                    },
                },
                "DownloadCapabilityRequest": {
                    "type": "object",
                    "required": ["purpose"],
                    "properties": {
                        "purpose": {
                            "type": "string",
                            "enum": ["original", "markdown"],
                        },
                        "ttlSecs": {"type": "integer", "minimum": 1},
                    },
                },
                "DownloadCapabilityResponse": {
                    "type": "object",
                    "required": [
                        "capabilityId",
                        "token",
                        "purpose",
                        "documentId",
                        "versionId",
                        "expiresAt",
                        "requestId",
                    ],
                    "properties": {
                        "capabilityId": {"type": "string", "format": "uuid"},
                        "token": {"type": "string", "writeOnly": True},
                        "purpose": {"type": "string"},
                        "documentId": {"type": "string", "format": "uuid"},
                        "versionId": {"type": "string", "format": "uuid"},
                        "expiresAt": {"type": "string", "format": "date-time"},
                        "requestId": {"type": "string", "format": "uuid"},
                    },
                },
                "DownloadRedeemRequest": {
                    "type": "object",
                    "required": ["token"],
                    "properties": {"token": {"type": "string", "writeOnly": True}},
                },
                "Conflict": {
                    "type": "object",
                    "required": [
                        "id",
                        "status",
                        "severity",
                        "conflictType",
                        "claimAId",
                        "claimBId",
                        "firstDetectedAt",
                        "createdAt",
                        "updatedAt",
                        "requestId",
                    ],
                    "properties": {
                        "id": {"type": "string", "format": "uuid"},
                        "status": {"type": "string"},
                        "severity": {"type": "string"},
                        "conflictType": {"type": "string"},
                        "claimAId": {"type": "string", "format": "uuid"},
                        "claimBId": {"type": "string", "format": "uuid"},
                        "firstDetectedAt": {"type": "string", "format": "date-time"},
                        "firstDetectedVersionId": {
                            "type": ["string", "null"],
                            "format": "uuid",
                        },
                        "resolvedAt": {
                            "type": ["string", "null"],
                            "format": "date-time",
                        },
                        "resolutionNote": {"type": ["string", "null"]},
                        "resolutionVersionAId": {
                            "type": ["string", "null"],
                            "format": "uuid",
                        },
                        "resolutionVersionBId": {
                            "type": ["string", "null"],
                            "format": "uuid",
                        },
                        "createdAt": {"type": "string", "format": "date-time"},
                        "updatedAt": {"type": "string", "format": "date-time"},
                        "requestId": {"type": "string", "format": "uuid"},
                    },
                },
                "ConflictList": {
                    "type": "object",
                    "required": ["items", "pageInfo", "requestId"],
                    "properties": {
                        "items": {
                            "type": "array",
                            "items": {"$ref": "#/components/schemas/Conflict"},
                        },
                        "pageInfo": {"$ref": "#/components/schemas/PageInfo"},
                        "requestId": {"type": "string", "format": "uuid"},
                    },
                },
                "ConflictTriageRequest": {
                    "type": "object",
                    "required": ["status"],
                    "properties": {
                        "status": {"type": "string"},
                        "resolutionNote": {"type": "string"},
                        "resolutionVersionAId": {
                            "type": "string",
                            "format": "uuid",
                        },
                        "resolutionVersionBId": {
                            "type": "string",
                            "format": "uuid",
                        },
                    },
                },
                "ConflictEvidence": {
                    "type": "object",
                    "required": [
                        "id",
                        "conflictId",
                        "claimId",
                        "evidenceRole",
                        "createdAt",
                    ],
                    "properties": {
                        "id": {"type": "string", "format": "uuid"},
                        "conflictId": {"type": "string", "format": "uuid"},
                        "claimId": {"type": "string", "format": "uuid"},
                        "evidenceRole": {"type": "string"},
                        "citationQuote": {"type": ["string", "null"]},
                        "createdAt": {"type": "string", "format": "date-time"},
                    },
                },
                "ConflictEvidenceList": {
                    "type": "object",
                    "required": ["items", "pageInfo", "requestId"],
                    "properties": {
                        "items": {
                            "type": "array",
                            "items": {"$ref": "#/components/schemas/ConflictEvidence"},
                        },
                        "pageInfo": {"$ref": "#/components/schemas/PageInfo"},
                        "requestId": {"type": "string", "format": "uuid"},
                    },
                },
                "CitationResolveItem": {
                    "type": "object",
                    "required": ["chunkId"],
                    "properties": {
                        "chunkId": {"type": "string", "format": "uuid"},
                        "expectedVersionId": {"type": "string", "format": "uuid"},
                        "expectedDocumentId": {"type": "string", "format": "uuid"},
                        "expectedContentSha256": {"type": "string"},
                        "expectedQuote": {"type": "string"},
                        "expectedSpanStart": {"type": "integer", "minimum": 0},
                        "expectedSpanEnd": {"type": "integer", "minimum": 0},
                    },
                },
                "StableCitation": {
                    "type": "object",
                    "required": [
                        "orgId",
                        "logicalDocumentId",
                        "versionId",
                        "versionNumber",
                        "contentSha256",
                        "chunkId",
                        "chunkIdentitySha256",
                        "spanStart",
                        "spanEnd",
                        "quote",
                        "effectiveFrom",
                        "isCurrent",
                        "heading",
                    ],
                    "properties": {
                        "orgId": {"type": "string", "format": "uuid"},
                        "logicalDocumentId": {"type": "string", "format": "uuid"},
                        "versionId": {"type": "string", "format": "uuid"},
                        "versionNumber": {"type": "integer"},
                        "contentSha256": {"type": "string"},
                        "chunkId": {"type": "string", "format": "uuid"},
                        "chunkIdentitySha256": {"type": "string"},
                        "page": {"type": "integer"},
                        "slide": {"type": "integer"},
                        "sheet": {"type": "string"},
                        "spanStart": {"type": "integer"},
                        "spanEnd": {"type": "integer"},
                        "quote": {"type": "string"},
                        "effectiveFrom": {"type": "string", "format": "date-time"},
                        "effectiveTo": {"type": ["string", "null"], "format": "date-time"},
                        "isCurrent": {"type": "boolean"},
                        "heading": {"type": "string"},
                    },
                },
                "CitationResolveRequest": {
                    "type": "object",
                    "required": ["citations"],
                    "properties": {
                        "citations": {
                            "type": "array",
                            "items": {"$ref": "#/components/schemas/CitationResolveItem"},
                        }
                    },
                },
                "CitationResolveResponse": {
                    "type": "object",
                    "required": ["citations", "requestId"],
                    "properties": {
                        "citations": {
                            "type": "array",
                            "items": {"$ref": "#/components/schemas/StableCitation"},
                        },
                        "requestId": {"type": "string", "format": "uuid"},
                    },
                },
                "Job": {
                    "type": "object",
                    "required": [
                        "id",
                        "jobType",
                        "status",
                        "attempts",
                        "maxAttempts",
                        "availableAt",
                        "createdAt",
                        "updatedAt",
                        "requestId",
                    ],
                    "properties": {
                        "id": {"type": "string", "format": "uuid"},
                        "jobType": {"type": "string"},
                        "status": {"type": "string"},
                        "attempts": {"type": "integer"},
                        "maxAttempts": {"type": "integer"},
                        "documentId": {"type": ["string", "null"], "format": "uuid"},
                        "versionId": {"type": ["string", "null"], "format": "uuid"},
                        "availableAt": {"type": "string", "format": "date-time"},
                        "startedAt": {
                            "type": ["string", "null"],
                            "format": "date-time",
                        },
                        "finishedAt": {
                            "type": ["string", "null"],
                            "format": "date-time",
                        },
                        "lastError": {"type": ["string", "null"]},
                        "createdAt": {"type": "string", "format": "date-time"},
                        "updatedAt": {"type": "string", "format": "date-time"},
                        "requestId": {"type": "string", "format": "uuid"},
                    },
                },
                "JobList": {
                    "type": "object",
                    "required": ["items", "pageInfo", "requestId"],
                    "properties": {
                        "items": {
                            "type": "array",
                            "items": {"$ref": "#/components/schemas/Job"},
                        },
                        "pageInfo": {"$ref": "#/components/schemas/PageInfo"},
                        "requestId": {"type": "string", "format": "uuid"},
                    },
                },
                "UploadResponse": {
                    "type": "object",
                    "required": [
                        "disposition",
                        "objectId",
                        "sha256",
                        "sizeBytes",
                        "canonicalFormat",
                        "requestId",
                    ],
                    "properties": {
                        "disposition": {"type": "string"},
                        "threatClass": {"type": "string"},
                        "reasonCode": {"type": "string"},
                        "objectId": {
                            "type": "string",
                            "format": "uuid",
                            "description": "Opaque upload identity (not a storage key)",
                        },
                        "sha256": {"type": "string"},
                        "sizeBytes": {"type": "integer"},
                        "canonicalFormat": {"type": "string"},
                        "originalFilename": {"type": "string"},
                        "requestId": {"type": "string", "format": "uuid"},
                    },
                },
                "VersionMode": {
                    "type": "object",
                    "required": ["type"],
                    "properties": {
                        "type": {
                            "type": "string",
                            "enum": ["current", "as_of", "compare", "history"],
                        },
                        "at": {"type": "string", "format": "date-time"},
                        "documentId": {"type": "string", "format": "uuid"},
                        "versionA": {"type": "string", "format": "uuid"},
                        "versionB": {"type": "string", "format": "uuid"},
                    },
                },
                "SearchRequest": {
                    "type": "object",
                    "required": ["query"],
                    "properties": {
                        "query": {"type": "string"},
                        "collectionIds": {
                            "type": "array",
                            "items": {"type": "string", "format": "uuid"},
                        },
                        "mode": {"$ref": "#/components/schemas/VersionMode"},
                        "limit": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 100,
                        },
                    },
                },
                "SearchHitLocator": {
                    "type": "object",
                    "properties": {
                        "page": {"type": ["integer", "null"]},
                        "slide": {"type": ["integer", "null"]},
                        "sheet": {"type": ["string", "null"]},
                        "spanStart": {"type": "integer"},
                        "spanEnd": {"type": "integer"},
                    },
                },
                "SearchHit": {
                    "type": "object",
                    "required": [
                        "chunkId",
                        "collectionId",
                        "documentId",
                        "versionId",
                        "versionNumber",
                        "contentSha256",
                        "heading",
                        "snippet",
                        "score",
                        "isCurrent",
                        "locator",
                    ],
                    "properties": {
                        "chunkId": {"type": "string", "format": "uuid"},
                        "collectionId": {"type": "string", "format": "uuid"},
                        "documentId": {"type": "string", "format": "uuid"},
                        "versionId": {"type": "string", "format": "uuid"},
                        "versionNumber": {"type": "integer"},
                        "contentSha256": {"type": "string"},
                        "heading": {"type": "string"},
                        "snippet": {"type": "string"},
                        "score": {"type": "number"},
                        "isCurrent": {"type": "boolean"},
                        "locator": {"$ref": "#/components/schemas/SearchHitLocator"},
                    },
                },
                "SearchResponse": {
                    "type": "object",
                    "required": ["hits", "warnings", "embeddingMode", "requestId"],
                    "properties": {
                        "hits": {
                            "type": "array",
                            "items": {"$ref": "#/components/schemas/SearchHit"},
                        },
                        "warnings": {"type": "array", "items": {"type": "string"}},
                        "embeddingMode": {"type": "string"},
                        "requestId": {"type": "string", "format": "uuid"},
                    },
                },
                "AskRequest": {
                    "type": "object",
                    "required": ["question"],
                    "properties": {
                        "question": {"type": "string"},
                        "collectionIds": {
                            "type": "array",
                            "items": {"type": "string", "format": "uuid"},
                        },
                        "mode": {"$ref": "#/components/schemas/VersionMode"},
                        "limit": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 32,
                        },
                        "useProvider": {"type": "boolean"},
                    },
                },
                "AskCitation": {
                    "type": "object",
                    "required": [
                        "citeId",
                        "documentId",
                        "versionId",
                        "versionNumber",
                        "contentSha256",
                        "chunkId",
                        "isCurrent",
                        "heading",
                        "quote",
                    ],
                    "properties": {
                        "citeId": {"type": "string"},
                        "documentId": {"type": "string", "format": "uuid"},
                        "versionId": {"type": "string", "format": "uuid"},
                        "versionNumber": {"type": "integer"},
                        "contentSha256": {"type": "string"},
                        "chunkId": {"type": "string", "format": "uuid"},
                        "isCurrent": {"type": "boolean"},
                        "heading": {"type": "string"},
                        "quote": {"type": "string"},
                    },
                },
                "AskResponse": {
                    "type": "object",
                    "required": ["answer", "citations", "mode", "grounded", "requestId"],
                    "properties": {
                        "answer": {"type": "string"},
                        "citations": {
                            "type": "array",
                            "items": {"$ref": "#/components/schemas/AskCitation"},
                        },
                        "mode": {"type": "string"},
                        "grounded": {"type": "boolean"},
                        "warnings": {"type": "array", "items": {"type": "string"}},
                        "versionContext": {"type": "object"},
                        "conflictWarnings": {"type": "array", "items": {"type": "object"}},
                        "requestId": {"type": "string", "format": "uuid"},
                    },
                },
                "SseEnvelope": {
                    "type": "object",
                    "description": "Canonical JSON object in each text/event-stream data frame (repeated frames, not one JSON body).",
                    "required": ["version", "sequence", "event", "requestId", "data"],
                    "properties": {
                        "version": {"type": "integer"},
                        "sequence": {"type": "integer"},
                        "event": {
                            "type": "string",
                            "enum": ["metadata", "token", "close", "error"],
                        },
                        "requestId": {"type": "string", "format": "uuid"},
                        "data": {},
                    },
                },
            },
        },
    }


    ensure_implicit_heads(paths)
    ensure_common_error_responses(paths)
    out = ROOT / "openapi.yaml"
    text = yaml.dump(doc, sort_keys=False, allow_unicode=True, width=100)
    out.write_text(text)
    print(f"wrote {out} ({len(paths)} paths)")


def ensure_implicit_heads(paths: dict) -> None:
    """Axum GET routes accept HEAD; document them for every GET operation."""
    for methods in paths.values():
        if "get" in methods and "head" not in methods:
            head = copy.deepcopy(methods["get"])
            head["operationId"] = methods["get"]["operationId"] + "Head"
            head["summary"] = methods["get"].get("summary", "") + " (HEAD)"
            methods["head"] = head


def ensure_common_error_responses(paths: dict) -> None:
    """Attach shared 401/403/429/503 responses used by the live middleware stack."""
    rate_limited = {"$ref": "#/components/responses/RateLimited"}
    api_error = {"$ref": "#/components/responses/ApiError"}
    for path, methods in paths.items():
        for method, op_body in methods.items():
            if method.startswith("x-") or not isinstance(op_body, dict):
                continue
            responses = op_body.setdefault("responses", {})
            responses.setdefault("429", rate_limited)
            if op_body.get("security"):
                responses.setdefault("401", api_error)
                responses.setdefault("403", api_error)
            if path in {
                "/ready",
                "/startup",
                "/api/v1/health/ready",
                "/api/v1/health/startup",
            } or path.endswith("/ready") or path.endswith("/startup"):
                responses.setdefault("503", api_error)


if __name__ == "__main__":
    main()
