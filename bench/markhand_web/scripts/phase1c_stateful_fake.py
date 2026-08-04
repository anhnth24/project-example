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
    _audit_entries: list[dict[str, Any]] = field(default_factory=list)
    _membership_active: bool = True

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
    ):
        probes_mod = self._probes()
        return probes_mod.HttpResponse(
            status=status,
            body=body,
            headers={
                "content-type": content_type,
                "x-request-id": _mint_server_request_id(),
            },
        )

    def _http_request(self, **kwargs: Any):
        probes_mod = self._probes()
        method = str(kwargs.get("method") or "GET").upper()
        path = str(kwargs.get("path") or "")
        token = kwargs.get("token")
        body = kwargs.get("body") or {}
        content_type = str(kwargs.get("content_type") or "application/json")
        multipart_body = kwargs.get("multipart_body")

        if self.force_all_403:
            return self._response(status=403, body='{"code":"forbidden"}')

        if token is None:
            return self._response(status=401, body='{"code":"unauthorized"}')

        if token == "unrelated-token-value":
            return self._response(status=401, body='{"code":"unauthorized"}')

        if path.endswith("/api/v1/auth/refresh"):
            refresh = body.get("refreshToken")
            if refresh == self.credentials.beta_refresh_token:
                return self._response(
                    status=200,
                    body=json.dumps({"accessToken": "rotated-access", "refreshToken": "rotated-refresh"}),
                )
            return self._response(status=401, body='{"code":"unauthorized"}')

        if path.endswith("/api/v1/auth/me"):
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
            return self._response(status=401, body='{"code":"unauthorized"}')

        if token == self.credentials.alpha_access_token:
            if self.accept_5xx:
                return self._response(status=500, body='{"code":"error"}')
            return self._response(status=403, body='{"code":"forbidden"}')

        if "/api/v1/members/" in path and method == "DELETE":
            self._membership_active = False
            return self._response(status=204, body="")

        if token == self.credentials.beta_access_token:
            if self.skip_owner_control:
                return self._response(status=403, body='{"code":"forbidden"}')

            if self.force_missing_resource:
                return self._response(status=404, body='{"code":"not_found"}')

            if path.endswith("/api/v1/ask/stream") and method == "POST":
                return self._response(
                    status=200,
                    body="event: message\ndata: {}\n\n",
                    content_type="text/event-stream",
                )

            if path.endswith("/events") and method == "GET":
                return self._response(
                    status=200,
                    body="event: status\ndata: {\"state\":\"queued\"}\n\n",
                    content_type="text/event-stream",
                )

            if path.endswith(f"/api/v1/downloads/{self.credentials.beta_download_capability}"):
                return self._response(status=200, body=self.seed.marker_beta, content_type="text/plain")

            if path.endswith("/api/v1/uploads") and method == "POST" and multipart_body is not None:
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

            if "/diff" in path and method == "GET":
                return self._response(
                    status=200,
                    body=json.dumps(
                        {
                            "fromVersionId": self.seed.beta_version_id,
                            "toVersionId": self.seed.beta_version_id,
                        }
                    ),
                )

            if path.endswith("/preview") and method == "GET":
                return self._response(
                    status=200,
                    body=json.dumps({"documentId": self.seed.beta_document_id}),
                )

            if path.endswith("/evidence") and method == "GET":
                return self._response(status=200, body=json.dumps({"items": []}))

            if path.endswith(f"/api/v1/collections/{self.seed.beta_collection_id}") and method == "GET":
                return self._response(
                    status=200,
                    body=json.dumps({"id": self.seed.beta_collection_id, "name": self.seed.marker_beta}),
                )

            if (
                path.endswith(f"/api/v1/collections/{self.seed.beta_denial_disposable_collection_id}")
                and method in {"GET", "PATCH", "DELETE"}
            ):
                payload = {"id": self.seed.beta_denial_disposable_collection_id, "name": "disposable"}
                status = 200 if method != "DELETE" else 204
                return self._response(status=status, body=json.dumps(payload) if method != "DELETE" else "")

            if "/versions/" in path and method == "GET":
                return self._response(
                    status=200,
                    body=json.dumps({"id": self.seed.beta_version_id, "versionId": self.seed.beta_version_id}),
                )

            if path.endswith("/documents") and "/collections/" in path and method == "GET":
                return self._response(status=200, body=json.dumps({"items": []}))

            if path.endswith("/versions") and "/documents/" in path and method == "GET":
                return self._response(status=200, body=json.dumps({"items": []}))

            if path.endswith(f"/api/v1/documents/{self.seed.beta_document_id}") and method == "GET":
                return self._response(status=200, body=json.dumps({"id": self.seed.beta_document_id}))

            disposable_doc = getattr(self.seed, "beta_denial_disposable_document_id", "") or self.seed.beta_document_id
            if path.endswith(f"/api/v1/documents/{disposable_doc}") and method == "DELETE":
                return self._response(status=204, body="")

            if path.endswith(f"/api/v1/jobs/{self.seed.beta_job_id}") and method == "GET":
                return self._response(status=200, body=json.dumps({"id": self.seed.beta_job_id}))

            if path.endswith(f"/api/v1/conflicts/{self.seed.beta_conflict_id}") and method == "GET":
                return self._response(status=200, body=json.dumps({"id": self.seed.beta_conflict_id}))

            if path.endswith("/api/v1/conflicts") and method == "GET":
                return self._response(status=200, body=json.dumps({"items": [{"id": self.seed.beta_conflict_id}]}))

            if path.endswith(f"/api/v1/chat-sessions/{self.seed.beta_chat_session_id}") and method == "GET":
                return self._response(status=200, body=json.dumps({"id": self.seed.beta_chat_session_id}))

            disposable_chat = getattr(self.seed, "beta_denial_disposable_chat_session_id", "") or self.seed.beta_chat_session_id
            if path.endswith(f"/api/v1/chat-sessions/{disposable_chat}") and method == "DELETE":
                return self._response(status=204, body="")

            if path.endswith(f"/api/v1/projects/{self.seed.beta_project_id}") and method == "GET":
                return self._response(status=200, body=json.dumps({"id": self.seed.beta_project_id}))

            if path.endswith("/api/v1/search") or path.endswith("/api/v1/ask"):
                return self._response(status=200, body=json.dumps({"items": []}))

            if path.endswith("/api/v1/collections") and method == "GET":
                return self._response(status=200, body=json.dumps({"items": [{"id": self.seed.beta_collection_id}]}))

            if path.endswith("/api/v1/collections") and method == "POST":
                new_id = str(uuid.uuid4())
                return self._response(status=201, body=json.dumps({"id": new_id}))

            if path.endswith("/api/v1/graph") and method == "GET":
                return self._response(status=200, body=json.dumps({"nodes": []}))

            if path.endswith("/api/v1/usage") and method == "GET":
                return self._response(status=200, body=json.dumps({"documents": 0}))

            if method in {"POST", "PATCH", "DELETE"}:
                return self._response(status=204 if method == "DELETE" else 200, body="{}" if method != "DELETE" else "")

            if method == "GET":
                return self._response(status=200, body=json.dumps({"id": "ok", "items": []}))

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
        foreign_count = sum(1 for item in report.observations if item.scenario == "foreign")
        owner_count = sum(1 for item in report.observations if item.scenario == "owner_control")
        return {
            "foreignCount": foreign_count,
            "ownerControlCount": owner_count,
            "executableHttpSseCount": len(mapping),
        }

    @staticmethod
    def assert_credentials_purged(path: Path) -> None:
        if path.exists():
            raise RuntimeError("credentials survived cleanup")
