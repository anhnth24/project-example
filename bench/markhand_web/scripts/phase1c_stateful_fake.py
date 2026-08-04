#!/usr/bin/env python3
"""Stateful fake deployment for Phase 1C HTTP slice unit tests."""

from __future__ import annotations

import importlib.util
import json
import sys
import uuid
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[3]


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"missing module {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def _mint_server_request_id() -> str:
    return str(uuid.uuid4())


@dataclass
class StatefulFakeDeployment:
    seed: Any
    credentials: Any
    skip_owner_control: bool = False
    force_all_403: bool = False
    force_missing_resource: bool = False
    accept_5xx: bool = False
    _membership_active: bool = True
    _beta_alpha_editor: bool = True
    _deleted_collections: set[str] = field(default_factory=set)
    _deleted_documents: set[str] = field(default_factory=set)
    _deleted_chat_sessions: set[str] = field(default_factory=set)
    _deleted_members: set[str] = field(default_factory=set)
    _revoked_invites: set[str] = field(default_factory=set)
    _triaged_conflicts: set[str] = field(default_factory=set)
    _accepted_invites: set[str] = field(default_factory=set)
    _revoked_refresh_tokens: set[str] = field(default_factory=set)
    _member_roles: dict[str, str] = field(default_factory=dict)
    _collection_names: dict[str, str] = field(default_factory=dict)

    def _probes(self):
        return _load("phase1c_deployed_probes_fake", ROOT / "bench/markhand_web/scripts/phase1c_deployed_probes.py")

    def _denial(self):
        return _load("phase1c_http_denial_fake", ROOT / "bench/markhand_web/scripts/phase1c_http_denial.py")

    def _response(
        self,
        *,
        status: int,
        body: str,
        content_type: str = "application/json",
        request_id: str | None = None,
    ):
        probes_mod = self._probes()
        resolved_request_id = request_id or _mint_server_request_id()
        return probes_mod.HttpResponse(
            status=status,
            body=body,
            headers={
                "content-type": content_type,
                "x-request-id": resolved_request_id,
            },
        )

    def _resolve_operation_id(self, path: str, method: str) -> str:
        mapping = self._denial().build_http_sse_denial_mapping()
        for entry in mapping:
            if entry.method.upper() != method:
                continue
            template_path = self._denial().API_PREFIX + entry.path_template
            for key, value in {
                "orgId": self.seed.org_beta_id,
                "collectionId": self.seed.beta_collection_id,
                "documentId": self.seed.beta_document_id,
                "jobId": self.seed.beta_job_id,
                "userId": getattr(self.credentials, "beta_denial_disposable_member_user_id", ""),
                "sessionId": self.seed.beta_chat_session_id,
                "projectId": self.seed.beta_project_id,
                "versionId": self.seed.beta_version_id,
                "conflictId": self.seed.beta_conflict_id,
                "inviteId": getattr(self.credentials, "beta_denial_disposable_invite_id", ""),
                "capability": self.credentials.beta_download_capability,
            }.items():
                template_path = template_path.replace("{" + key + "}", value)
            if template_path.split("?", 1)[0] == path.split("?", 1)[0]:
                return entry.operation_id
        return ""

    def _owner_mutation_success(
        self,
        *,
        operation_id: str,
        path: str,
        method: str,
        body: dict[str, Any] | None,
    ) -> Any | None:
        request_id = _mint_server_request_id()
        if operation_id == "appendChatTurn":
            return self._response(status=201, body=json.dumps({"id": request_id, "seq": 1, "requestId": request_id}))
        if operation_id == "updateChatSession":
            session_id = path.rstrip("/").split("/")[-1]
            return self._response(status=200, body=json.dumps({"id": session_id, "title": (body or {}).get("title", "updated")}))
        if operation_id == "createChatSession":
            return self._response(status=201, body=json.dumps({"id": self.seed.beta_chat_session_id, "title": (body or {}).get("title", "chat")}))
        if operation_id == "createCollection":
            return self._response(status=201, body=json.dumps({"id": self.seed.beta_collection_id, "requestId": request_id}))
        if operation_id == "createProject":
            return self._response(status=201, body=json.dumps({"id": self.seed.beta_project_id, "requestId": request_id}))
        if operation_id == "createOrg":
            return self._response(status=201, body=json.dumps({"id": self.seed.org_beta_id, "requestId": request_id}))
        if operation_id == "createMemberInvite":
            return self._response(
                status=201,
                body=json.dumps(
                    {
                        "invite": {"id": request_id, "email": (body or {}).get("email", "x@example.com"), "role": "viewer", "status": "pending"},
                        "token": "mhinv1.owner-created",
                        "requestId": request_id,
                    }
                ),
            )
        if operation_id == "assignCollectionProject":
            return self._response(status=200, body=json.dumps({"id": self.seed.beta_collection_id, "requestId": request_id}))
        if operation_id in {"publishDocumentVersion", "reindexDocument", "approveIntake"}:
            return self._response(status=200, body=json.dumps({"requestId": request_id}))
        if operation_id == "issueDownloadCapability":
            return self._response(status=200, body=json.dumps({"capability": self.credentials.beta_download_capability, "requestId": request_id}))
        if operation_id == "updateCollection":
            return self._response(status=200, body=json.dumps({"id": self.seed.beta_collection_id, "name": (body or {}).get("name", "updated")}))
        if operation_id == "updateProject":
            return self._response(status=200, body=json.dumps({"id": self.seed.beta_project_id, "name": (body or {}).get("name", "updated")}))
        if operation_id == "switchOrg":
            return self._response(status=200, body=json.dumps({"accessToken": self.credentials.alpha_beta_access_token, "refreshToken": "r", "requestId": request_id}))
        if operation_id in {"search", "ask"}:
            return self._response(status=200, body=json.dumps({"items": [], "requestId": request_id}))
        if operation_id == "resolveCitation":
            return None
        return None

    def _owner_success(self, *, operation_id: str, path: str, method: str) -> Any | None:
        denial_mod = self._denial()
        if path.endswith("/api/v1/usage") and method == "GET":
            return self._response(status=200, body=json.dumps({"items": []}))
        if path.endswith("/api/v1/members") and method == "GET" and path.count("/") == 3:
            disposable_member = getattr(self.credentials, "beta_denial_disposable_member_user_id", "")
            delete_member = getattr(self.credentials, "beta_denial_disposable_delete_member_user_id", "")
            items: list[dict[str, Any]] = []
            if disposable_member and disposable_member not in self._deleted_members:
                items.append(
                    {
                        "userId": disposable_member,
                        "role": self._member_roles.get(disposable_member, "editor"),
                        "email": "phase1c-disposable@example.com",
                        "displayName": "Disposable",
                        "state": "active",
                    }
                )
            if delete_member and delete_member not in self._deleted_members:
                items.append(
                    {
                        "userId": delete_member,
                        "role": "viewer",
                        "email": "phase1c-delete-member@poc.example",
                        "displayName": "Delete Member",
                        "state": "active",
                    }
                )
            if "55555555-5555-5555-5555-555555555501" not in self._deleted_members:
                items.append(
                    {
                        "userId": "55555555-5555-5555-5555-555555555501",
                        "role": "viewer",
                        "email": "phase1c-accept@poc.example",
                        "displayName": "Accept",
                        "state": "active",
                    }
                )
            return self._response(status=200, body=json.dumps({"items": items, "page": {"hasMore": False}}))
        if path.endswith("/api/v1/members/invites") and method == "GET":
            invite_id = getattr(self.credentials, "beta_denial_disposable_invite_id", "")
            return self._response(
                status=200,
                body=json.dumps(
                    {
                        "items": [
                            {
                                "id": invite_id,
                                "status": "revoked" if invite_id in self._revoked_invites else "pending",
                            }
                        ]
                    }
                ),
            )
        disposable_member = getattr(self.credentials, "beta_denial_disposable_member_user_id", "")
        delete_member = getattr(self.credentials, "beta_denial_disposable_delete_member_user_id", "")
        if "/chat-sessions/" in path and method == "GET" and not path.endswith("/turns"):
            session_id = path.rstrip("/").split("/")[-1]
            if session_id in self._deleted_chat_sessions:
                return self._response(status=404, body='{"code":"not_found"}')
            return self._response(
                status=200,
                body=json.dumps(
                    {
                        "session": {
                            "id": session_id,
                            "title": "phase1c-owner-chat",
                            "createdAt": "2026-08-04T00:00:00Z",
                            "updatedAt": "2026-08-04T00:00:00Z",
                        },
                        "turns": [],
                    }
                ),
            )
        if "/versions/" in path and "/diff" in path and method == "GET":
            version_id = self.seed.beta_version_id
            return self._response(
                status=200,
                body=json.dumps(
                    {
                        "documentId": self.seed.beta_document_id,
                        "left": {"id": version_id, "documentId": self.seed.beta_document_id, "versionNumber": 1},
                        "right": {"id": version_id, "documentId": self.seed.beta_document_id, "versionNumber": 1},
                        "note": "identity diff",
                        "requestId": _mint_server_request_id(),
                    }
                ),
            )
        if "/versions/" in path and method == "GET":
            return self._response(
                status=200,
                body=json.dumps(
                    {
                        "id": self.seed.beta_version_id,
                        "documentId": self.seed.beta_document_id,
                        "versionNumber": 1,
                        "isCurrent": True,
                        "sourceContentSha256": "a" * 64,
                    }
                ),
            )
        if path.endswith("/api/v1/ask/stream") and method == "POST":
            request_id = _mint_server_request_id()
            if not self._beta_alpha_editor:
                envelope = {
                    "version": 1,
                    "sequence": 1,
                    "event": "stream.closed",
                    "requestId": request_id,
                    "data": {"reason": "principal_denied"},
                }
                return self._response(
                    status=200,
                    body=(
                        f"id: 1\n"
                        f"event: stream.closed\n"
                        f"data: {json.dumps(envelope)}\n\n"
                    ),
                    content_type="text/event-stream",
                    request_id=request_id,
                )
            token_env = {
                "version": 1,
                "sequence": 1,
                "event": "ask.token",
                "requestId": request_id,
                "data": {"text": "ok"},
            }
            close_env = {
                "version": 1,
                "sequence": 2,
                "event": denial_mod.SSE_TERMINAL_EVENT,
                "requestId": request_id,
                "data": {"reason": "done"},
            }
            return self._response(
                status=200,
                body=(
                    f"id: 1\n"
                    f"event: ask.token\n"
                    f"data: {json.dumps(token_env)}\n\n"
                    f"id: 2\n"
                    f"event: {denial_mod.SSE_TERMINAL_EVENT}\n"
                    f"data: {json.dumps(close_env)}\n\n"
                ),
                content_type="text/event-stream",
                request_id=request_id,
            )
        if path.endswith("/events") and method == "GET":
            request_id = _mint_server_request_id()
            status_env = {
                "version": 1,
                "sequence": 1,
                "event": "status",
                "requestId": request_id,
                "data": {"state": "queued"},
            }
            close_env = {
                "version": 1,
                "sequence": 2,
                "event": denial_mod.SSE_TERMINAL_EVENT,
                "requestId": request_id,
                "data": {"reason": "done"},
            }
            return self._response(
                status=200,
                body=(
                    f"id: 1\n"
                    f"event: status\n"
                    f"data: {json.dumps(status_env)}\n\n"
                    f"id: 2\n"
                    f"event: {denial_mod.SSE_TERMINAL_EVENT}\n"
                    f"data: {json.dumps(close_env)}\n\n"
                ),
                content_type="text/event-stream",
                request_id=request_id,
            )
        if path.endswith(f"/api/v1/collections/{self.seed.beta_collection_id}") and method == "GET":
            return self._response(
                status=200,
                body=json.dumps({"id": self.seed.beta_collection_id, "name": self.seed.marker_beta}),
            )
        disposable_update = getattr(self.credentials, "beta_denial_disposable_collection_update_id", "")
        if disposable_update and path.endswith(f"/api/v1/collections/{disposable_update}") and method == "GET":
            name = self._collection_names.get(disposable_update, "owner-updated")
            return self._response(status=200, body=json.dumps({"id": disposable_update, "name": name}))
        if path.endswith(f"/api/v1/documents/{self.seed.beta_document_id}") and method == "GET":
            return self._response(status=200, body=json.dumps({"id": self.seed.beta_document_id, "title": "beta"}))
        if path.endswith(f"/api/v1/documents/{self.seed.beta_document_id}/preview") and method == "GET":
            return self._response(
                status=200,
                body=json.dumps({"documentId": self.seed.beta_document_id, "versionId": self.seed.beta_version_id}),
            )
        if path.endswith("/api/v1/search") or path.endswith("/api/v1/ask"):
            return self._response(status=200, body=json.dumps({"items": []}))
        if path.endswith("/api/v1/conflicts") and method == "GET":
            return self._response(status=200, body=json.dumps({"items": [], "requestId": _mint_server_request_id()}))
        disposable_conflict = getattr(self.credentials, "beta_denial_disposable_conflict_id", "")
        beta_conflict = self.seed.beta_conflict_id
        if beta_conflict and path.endswith(f"/api/v1/conflicts/{beta_conflict}/evidence") and method == "GET":
            return self._response(status=200, body=json.dumps({"items": [], "requestId": _mint_server_request_id()}))
        if beta_conflict and path.endswith(f"/api/v1/conflicts/{beta_conflict}") and method == "GET":
            return self._response(status=200, body=json.dumps({"id": beta_conflict, "status": "open"}))
        if disposable_conflict and path.endswith(f"/api/v1/conflicts/{disposable_conflict}") and method == "GET":
            status = "resolved" if disposable_conflict in self._triaged_conflicts else "open"
            return self._response(status=200, body=json.dumps({"id": disposable_conflict, "status": status}))
        if path.endswith(f"/api/v1/jobs/{self.seed.beta_job_id}") and method == "GET":
            return self._response(status=200, body=json.dumps({"id": self.seed.beta_job_id, "status": "queued"}))
        if path.endswith("/api/v1/collections") and method == "GET" and "?" not in path:
            items = [{"id": self.seed.beta_collection_id, "name": self.seed.marker_beta}]
            if denial_mod.DUPLICATE_COLLECTION_NAME != self.seed.marker_beta:
                items.append({"id": self.seed.alpha_collection_id, "name": denial_mod.DUPLICATE_COLLECTION_NAME})
            return self._response(status=200, body=json.dumps({"items": items, "page": {"hasMore": False}}))
        if path.endswith("/api/v1/auth/me") and method == "GET":
            return self._response(
                status=200,
                body=json.dumps(
                    {
                        "userId": self.seed.alpha_user_id,
                        "orgId": self.seed.org_beta_id,
                        "sessionId": self.credentials.alpha_session_id,
                    }
                ),
            )
        if path.endswith("/api/v1/orgs") and method == "GET":
            return self._response(status=200, body=json.dumps({"items": [{"id": self.seed.org_beta_id, "name": "beta"}]}))
        if path.endswith(f"/api/v1/orgs/{self.seed.org_beta_id}") and method == "GET":
            return self._response(status=200, body=json.dumps({"id": self.seed.org_beta_id, "name": "beta"}))
        if path.endswith(f"/api/v1/collections/{self.seed.beta_collection_id}/documents") and method == "GET":
            return self._response(
                status=200,
                body=json.dumps({"items": [{"id": self.seed.beta_document_id, "title": "beta"}], "page": {"hasMore": False}}),
            )
        if path.endswith("/api/v1/documents") and method == "GET":
            return self._response(status=200, body=json.dumps({"items": [{"id": self.seed.beta_document_id, "title": "beta"}]}))
        if path.endswith("/api/v1/chat-sessions") and method == "GET":
            return self._response(status=200, body=json.dumps({"items": [{"id": self.seed.beta_chat_session_id, "title": "chat"}]}))
        if path.endswith("/api/v1/projects") and method == "GET":
            return self._response(status=200, body=json.dumps({"items": [{"id": self.seed.beta_project_id, "name": "project"}]}))
        if path.endswith("/api/v1/audit") and method == "GET":
            return self._response(status=200, body=json.dumps({"items": [], "page": {"hasMore": False}}))
        if path.endswith("/api/v1/graph") and method == "GET":
            return self._response(status=200, body=json.dumps({"nodes": [], "requestId": _mint_server_request_id()}))
        if "/versions" in path and path.endswith("/versions") and method == "GET":
            return self._response(status=200, body=json.dumps({"items": [{"id": self.seed.beta_version_id}]}))
        return None

    def _unknown_owner_mapping(self, *, method: str, path: str) -> Any:
        raise RuntimeError(f"stateful fake missing explicit handler: {method} {path}")

    def _http_request(self, **kwargs: Any):
        method = str(kwargs.get("method") or "GET").upper()
        path = str(kwargs.get("path") or "")
        if method not in {"GET", "POST", "PATCH", "DELETE"}:
            return self._unknown_owner_mapping(method=method, path=path)
        token = kwargs.get("token")
        body = kwargs.get("body") or {}
        multipart_body = kwargs.get("multipart_body")

        if self.force_all_403:
            return self._response(status=403, body='{"code":"forbidden"}')

        if token is None:
            return self._response(status=401, body='{"code":"unauthorized"}')

        stale_access = getattr(self.credentials, "beta_denial_stale_access_token", "")
        if stale_access and token == stale_access:
            return self._response(status=401, body='{"code":"unauthorized"}')

        if path.endswith("/api/v1/auth/refresh"):
            refresh = body.get("refreshToken")
            if refresh in self._revoked_refresh_tokens:
                return self._response(status=401, body='{"code":"unauthorized"}')
            if refresh == self.credentials.beta_refresh_token:
                return self._response(
                    status=200,
                    body=json.dumps({"accessToken": "rotated-access", "refreshToken": "rotated-refresh"}),
                )
            return self._response(status=401, body='{"code":"unauthorized"}')

        if path.endswith("/api/v1/auth/me"):
            if token in self._revoked_refresh_tokens or token == "rotated-access":
                return self._response(status=401, body='{"code":"unauthorized"}')
            if token == self.credentials.beta_access_token:
                if not self._membership_active:
                    return self._response(status=403, body='{"code":"forbidden"}')
                return self._response(
                    status=200,
                    body=json.dumps(
                        {
                            "userId": self.seed.beta_user_id,
                            "orgId": self.seed.org_beta_id,
                            "sessionId": self.credentials.beta_session_id,
                        }
                    ),
                )
            if token == self.credentials.beta_alpha_access_token:
                if not self._membership_active or not self._beta_alpha_editor:
                    return self._response(status=403, body='{"code":"forbidden"}')
                return self._response(
                    status=200,
                    body=json.dumps(
                        {
                            "userId": self.seed.beta_user_id,
                            "orgId": self.seed.org_alpha_id,
                            "sessionId": self.credentials.beta_session_id,
                        }
                    ),
                )
            if token == self.credentials.alpha_access_token:
                return self._response(
                    status=200,
                    body=json.dumps(
                        {
                            "userId": self.seed.alpha_user_id,
                            "orgId": self.seed.org_alpha_id,
                            "sessionId": self.credentials.alpha_session_id,
                        }
                    ),
                )
            if token == getattr(self.credentials, "alpha_beta_access_token", ""):
                return self._response(
                    status=200,
                    body=json.dumps(
                        {
                            "userId": self.seed.alpha_user_id,
                            "orgId": self.seed.org_beta_id,
                            "sessionId": self.credentials.alpha_session_id,
                        }
                    ),
                )
            if token == getattr(self.credentials, "beta_denial_accept_access_token", ""):
                return self._response(
                    status=200,
                    body=json.dumps(
                        {
                            "userId": "55555555-5555-5555-5555-555555555501",
                            "orgId": self.seed.org_alpha_id,
                            "sessionId": self.credentials.alpha_session_id,
                        }
                    ),
                )
            return self._response(status=401, body='{"code":"unauthorized"}')

        if token == self.credentials.alpha_access_token:
            if self.seed.beta_member_user_id in path and method in {"PATCH", "DELETE"}:
                if method == "DELETE":
                    self._membership_active = False
                if method == "PATCH" and isinstance(body, dict) and body.get("role") == "viewer":
                    self._beta_alpha_editor = False
                return self._response(status=204 if method == "DELETE" else 200, body="" if method == "DELETE" else "{}")
            if path.endswith("/download-capability") and method == "POST" and self.seed.alpha_document_id in path:
                return self._response(status=200, body=json.dumps({"capability": "mhcap1.preview-cap"}))
            if self.accept_5xx:
                return self._response(status=500, body='{"code":"error"}')
            return self._response(status=403, body='{"code":"forbidden"}')

        for collection_id in list(self._deleted_collections):
            if collection_id in path:
                return self._response(status=404, body='{"code":"not_found"}')

        for document_id in list(self._deleted_documents):
            if document_id in path:
                return self._response(status=404, body='{"code":"not_found"}')

        for session_id in list(self._deleted_chat_sessions):
            if session_id in path:
                return self._response(status=404, body='{"code":"not_found"}')

        if path.endswith("/api/v1/orgs/switch") and method == "POST":
            org_id = body.get("orgId") if isinstance(body, dict) else None
            disposable = getattr(self.seed, "disposable_org_id", "")
            if org_id == disposable:
                return self._response(status=403, body='{"code":"membership_missing"}')
            if token in {
                getattr(self.credentials, "alpha_beta_access_token", ""),
                self.credentials.beta_access_token,
            }:
                return self._response(status=200, body=json.dumps({"accessToken": token, "refreshToken": "r"}))

        wrong_cap = getattr(self.credentials, "beta_denial_wrong_download_capability", "")
        if wrong_cap and path.endswith(f"/api/v1/downloads/{wrong_cap}"):
            return self._response(status=400, body='{"code":"validation_failed"}')

        if path.endswith("/api/v1/members/invites/accept") and method == "POST":
            invite_token = body.get("token") if isinstance(body, dict) else None
            negative = getattr(self.credentials, "beta_denial_negative_invite_token", "")
            owner = getattr(self.credentials, "beta_denial_accept_invite_token", "")
            if invite_token == negative:
                return self._response(status=404, body='{"code":"not_found"}')
            if invite_token == owner:
                if invite_token in self._accepted_invites:
                    return self._response(status=409, body='{"code":"conflict"}')
                self._accepted_invites.add(str(invite_token))
                return self._response(status=201, body=json.dumps({"userId": self.seed.beta_user_id, "role": "viewer"}))

        if token in {
            getattr(self.credentials, "alpha_beta_access_token", ""),
            getattr(self.credentials, "beta_denial_accept_access_token", ""),
            self.credentials.beta_alpha_access_token,
            self.credentials.beta_access_token,
        }:
            if self.skip_owner_control:
                return self._response(status=403, body='{"code":"forbidden"}')
            if self.force_missing_resource:
                return self._response(status=404, body='{"code":"not_found"}')
            if path.endswith("/api/v1/members/invites/accept") and method == "POST":
                invite_token = body.get("token")
                if invite_token in self._accepted_invites:
                    return self._response(status=409, body='{"code":"conflict"}')
                self._accepted_invites.add(str(invite_token))
                return self._response(status=201, body=json.dumps({"userId": self.seed.beta_user_id, "role": "viewer"}))
            if path.endswith("/api/v1/citations/resolve") and method == "POST":
                canonical = body.get("canonicalMarkdownSha256") if isinstance(body, dict) else ""
                version_id = body.get("versionId") if isinstance(body, dict) else ""
                logical_id = body.get("logicalDocumentId") if isinstance(body, dict) else ""
                if canonical == "f" * 64:
                    return self._response(status=403, body='{"code":"forbidden"}')
                if str(version_id) == "00000000-0000-0000-0000-000000000099":
                    return self._response(status=404, body='{"code":"not_found"}')
                if str(logical_id) == str(self.seed.alpha_document_id):
                    return self._response(status=404, body='{"code":"not_found"}')
                return self._response(
                    status=200,
                    body=json.dumps(
                        {
                            "citation": {
                                "citeId": "cite-1",
                                "logicalDocumentId": self.seed.beta_document_id,
                                "versionId": self.seed.beta_version_id,
                                "versionNumber": 1,
                                "sourceContentSha256": getattr(
                                    self.credentials, "beta_citation_source_content_sha256", "a" * 64
                                ),
                                "canonicalMarkdownSha256": getattr(
                                    self.credentials, "beta_citation_canonical_markdown_sha256", "b" * 64
                                ),
                                "quoteSha256": "c" * 64,
                                "chunkId": getattr(self.credentials, "beta_citation_chunk_id", self.seed.beta_document_id),
                                "chunkIdentitySha256": "d" * 64,
                                "page": None,
                                "slide": None,
                                "sheet": None,
                                "sourceSpanStart": 0,
                                "sourceSpanEnd": 1,
                                "quoteLocalStart": 0,
                                "quoteLocalEnd": 1,
                                "quote": getattr(self.credentials, "beta_citation_quote", "q"),
                                "isCurrent": True,
                                "anchor": "mhcite1.test",
                            },
                            "requestId": _mint_server_request_id(),
                        }
                    ),
                )
            if path.endswith(f"/api/v1/downloads/{self.credentials.beta_download_capability}"):
                return self._response(status=200, body=self.seed.marker_beta, content_type="text/plain")
            if path.endswith("/api/v1/uploads") and method == "POST" and multipart_body is not None:
                if token == self.credentials.beta_alpha_access_token and not self._beta_alpha_editor:
                    return self._response(status=403, body='{"code":"forbidden"}')
                return self._response(
                    status=201,
                    body=json.dumps(
                        {
                            "documentId": self.seed.beta_document_id,
                            "versionId": self.seed.beta_version_id,
                            "jobId": self.seed.beta_job_id,
                        }
                    ),
                )
            disposable_collection = getattr(self.credentials, "beta_denial_disposable_collection_id", "")
            disposable_update = getattr(self.credentials, "beta_denial_disposable_collection_update_id", "")
            disposable_doc = getattr(self.credentials, "beta_denial_disposable_document_id", "")
            disposable_chat = getattr(self.credentials, "beta_denial_disposable_chat_session_id", "")
            disposable_invite = getattr(self.credentials, "beta_denial_disposable_invite_id", "")
            disposable_conflict = getattr(self.credentials, "beta_denial_disposable_conflict_id", "")
            disposable_member = getattr(self.credentials, "beta_denial_disposable_member_user_id", "")
            delete_member = getattr(self.credentials, "beta_denial_disposable_delete_member_user_id", "")

            if disposable_member and path.endswith(f"/api/v1/members/{disposable_member}") and method == "PATCH":
                role = body.get("role") if isinstance(body, dict) else "viewer"
                self._member_roles[disposable_member] = str(role)
                return self._response(status=200, body="{}")
            if delete_member and path.endswith(f"/api/v1/members/{delete_member}") and method == "DELETE":
                self._deleted_members.add(delete_member)
                return self._response(status=204, body="")
            if disposable_collection and path.endswith(f"/api/v1/collections/{disposable_collection}") and method == "DELETE":
                if disposable_collection in self._deleted_collections:
                    return self._response(status=404, body='{"code":"not_found"}')
                self._deleted_collections.add(disposable_collection)
                return self._response(status=204, body="")
            if disposable_update and path.endswith(f"/api/v1/collections/{disposable_update}") and method == "PATCH":
                name = body.get("name", "updated") if isinstance(body, dict) else "updated"
                self._collection_names[disposable_update] = str(name)
                return self._response(status=200, body=json.dumps({"id": disposable_update, "name": name}))
            if disposable_doc and path.endswith(f"/api/v1/documents/{disposable_doc}") and method == "DELETE":
                if disposable_doc in self._deleted_documents:
                    return self._response(status=404, body='{"code":"not_found"}')
                self._deleted_documents.add(disposable_doc)
                return self._response(status=204, body="")
            if disposable_chat and path.endswith(f"/api/v1/chat-sessions/{disposable_chat}") and method == "DELETE":
                if disposable_chat in self._deleted_chat_sessions:
                    return self._response(status=404, body='{"code":"not_found"}')
                self._deleted_chat_sessions.add(disposable_chat)
                return self._response(status=204, body="")
            if disposable_invite and disposable_invite in path and method == "POST" and "revoke" in path:
                if disposable_invite in self._revoked_invites:
                    return self._response(status=409, body='{"code":"conflict"}')
                self._revoked_invites.add(disposable_invite)
                return self._response(status=204, body="")
            if disposable_conflict and path.endswith(f"/api/v1/conflicts/{disposable_conflict}/triage") and method == "POST":
                if disposable_conflict in self._triaged_conflicts:
                    return self._response(status=409, body='{"code":"conflict"}')
                self._triaged_conflicts.add(disposable_conflict)
                return self._response(
                    status=200,
                    body=json.dumps(
                        {
                            "id": disposable_conflict,
                            "status": "resolved",
                            "resolvedAt": "2026-08-04T00:00:00Z",
                            "requestId": _mint_server_request_id(),
                        }
                    ),
                )
            if path.endswith(f"/api/v1/collections/{self.seed.alpha_collection_id}") and method == "GET":
                return self._response(status=404, body='{"code":"not_found"}')
            operation_id = self._resolve_operation_id(path, method)
            handled = self._owner_success(operation_id=operation_id, path=path, method=method)
            if handled is not None:
                return handled
            mutation = self._owner_mutation_success(operation_id=operation_id, path=path, method=method, body=body if isinstance(body, dict) else None)
            if mutation is not None:
                return mutation
            return self._unknown_owner_mapping(method=method, path=path)

        if token == self.credentials.beta_alpha_access_token:
            if not self._membership_active or (multipart_body is not None and not self._beta_alpha_editor):
                return self._response(status=403, body='{"code":"forbidden"}')
            if path.endswith(f"/api/v1/collections/{self.seed.alpha_collection_id}") and method == "GET":
                return self._response(
                    status=200,
                    body=json.dumps({"id": self.seed.alpha_collection_id, "name": self.seed.marker_alpha}),
                )
            if path.endswith("/api/v1/uploads") and method == "POST" and multipart_body is not None:
                return self._response(
                    status=201,
                    body=json.dumps({"documentId": self.seed.alpha_document_id, "versionId": self.seed.alpha_version_id}),
                )

        if token == self.credentials.alpha_access_token:
            if self.accept_5xx:
                return self._response(status=500, body='{"code":"error"}')
            return self._response(status=403, body='{"code":"forbidden"}')

        return self._response(status=403, body='{"code":"forbidden"}')

    def run_denial_suite(self) -> dict[str, Any]:
        denial_mod = self._denial()
        git_sha = self.seed.source_revision.get("commit") or ("a" * 40)
        mapping = denial_mod.build_http_sse_denial_mapping()
        report = denial_mod.execute_http_denial_suite(
            seed=self.seed,
            credentials=self.credentials,
            http_request=self._http_request,
            api_base="http://fake",
            git_sha_full=git_sha,
        )
        if self.skip_owner_control:
            raise RuntimeError("skip_owner_control incompatible with execute_http_denial_suite")
        if report.failures and not self.accept_5xx:
            raise RuntimeError("; ".join(report.failures))
        negative_count = sum(
            1 for item in report.observations if item.scenario not in {"owner_control", "unauthenticated"}
        )
        owner_count = sum(1 for item in report.observations if item.scenario == "owner_control")
        return {
            "foreignCount": negative_count,
            "ownerControlCount": owner_count,
            "executableHttpSseCount": len(mapping),
        }

    @staticmethod
    def assert_credentials_purged(path: Path) -> None:
        if path.exists():
            raise RuntimeError("credentials survived cleanup")
