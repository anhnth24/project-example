#!/usr/bin/env python3
"""Stateful fake deployment for Phase 1C HTTP slice unit tests."""

from __future__ import annotations

import importlib.util
import json
import sys
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

    def _http_request(self, **kwargs: Any):
        probes_mod = self._probes()
        method = str(kwargs.get("method") or "GET")
        path = str(kwargs.get("path") or "")
        token = kwargs.get("token")
        supplied_request_id = kwargs.get("supplied_request_id")
        headers = {"content-type": kwargs.get("content_type") or "application/json"}
        if isinstance(supplied_request_id, str) and supplied_request_id.strip():
            headers["x-request-id"] = supplied_request_id.strip()

        if self.force_all_403:
            return probes_mod.HttpResponse(status=403, body='{"code":"forbidden"}', headers=headers)

        if token == "unrelated-token-value" or (token is None and path.endswith("/api/v1/auth/me")):
            return probes_mod.HttpResponse(status=401, body='{"code":"unauthorized"}', headers=headers)

        if path.endswith("/api/v1/auth/me") and token == self.credentials.beta_access_token:
            if not self._membership_active:
                return probes_mod.HttpResponse(status=403, body='{"code":"forbidden"}', headers=headers)
            return probes_mod.HttpResponse(
                status=200,
                body=json.dumps(
                    {
                        "userId": self.seed.beta_user_id,
                        "orgId": self.seed.org_alpha_id,
                        "sessionId": self.credentials.beta_session_id,
                    }
                ),
                headers=headers,
            )

        if path.endswith("/api/v1/auth/refresh"):
            refresh = (kwargs.get("body") or {}).get("refreshToken")
            if refresh == self.credentials.beta_refresh_token:
                return probes_mod.HttpResponse(
                    status=200,
                    body=json.dumps({"accessToken": "rotated-access", "refreshToken": "rotated-refresh"}),
                    headers=headers,
                )
            return probes_mod.HttpResponse(status=401, body='{"code":"unauthorized"}', headers=headers)

        if "/api/v1/members/" in path and method == "DELETE":
            self._membership_active = False
            return probes_mod.HttpResponse(status=204, body="", headers=headers)

        if path.endswith("/api/v1/orgs/switch") and method == "POST":
            session_target = "session-family-" + self.credentials.alpha_session_id[:8]
            self._audit_entries.append(
                {
                    "id": "audit-switch",
                    "action": "org.switch",
                    "targetType": "session",
                    "targetId": session_target,
                    "actorId": self.seed.alpha_user_id,
                    "outcome": "success",
                    "requestId": headers.get("x-request-id"),
                    "occurredAt": "2026-08-04T12:00:00Z",
                }
            )
            return probes_mod.HttpResponse(
                status=200,
                body=json.dumps({"switchSessionTargetId": session_target, "sessionId": session_target}),
                headers=headers,
            )

        if path.endswith("/api/v1/collections") and method == "POST":
            collection_id = "collection-" + self.seed.challenge[:8]
            self._audit_entries.append(
                {
                    "id": "audit-create",
                    "action": "collection.create",
                    "targetType": "collection",
                    "targetId": collection_id,
                    "actorId": self.seed.alpha_user_id,
                    "outcome": "success",
                    "requestId": headers.get("x-request-id"),
                    "occurredAt": "2026-08-04T12:00:01Z",
                }
            )
            return probes_mod.HttpResponse(status=201, body=json.dumps({"id": collection_id}), headers=headers)

        if path.endswith("/api/v1/audit"):
            return probes_mod.HttpResponse(
                status=200,
                body=json.dumps({"items": self._audit_entries, "page": {"hasMore": False, "nextCursor": None}}),
                headers=headers,
            )

        if self.force_missing_resource:
            return probes_mod.HttpResponse(status=404, body='{"code":"not_found"}', headers=headers)

        if token == self.credentials.alpha_access_token:
            if self.accept_5xx:
                return probes_mod.HttpResponse(status=500, body='{"code":"error"}', headers=headers)
            return probes_mod.HttpResponse(status=403, body='{"code":"forbidden"}', headers=headers)

        if token == self.credentials.beta_access_token:
            if self.skip_owner_control:
                return probes_mod.HttpResponse(status=403, body='{"code":"forbidden"}', headers=headers)
            if path.endswith(f"/api/v1/collections/{self.seed.beta_collection_id}"):
                return probes_mod.HttpResponse(
                    status=200,
                    body=json.dumps({"id": self.seed.beta_collection_id, "name": self.seed.marker_beta}),
                    headers=headers,
                )
            if path.endswith(f"/api/v1/documents/{self.seed.beta_document_id}"):
                return probes_mod.HttpResponse(status=200, body=json.dumps({"id": self.seed.beta_document_id}), headers=headers)
            if path.endswith(f"/api/v1/jobs/{self.seed.beta_job_id}"):
                return probes_mod.HttpResponse(status=200, body=json.dumps({"id": self.seed.beta_job_id}), headers=headers)
            if path.endswith("/api/v1/search") and method == "POST":
                return probes_mod.HttpResponse(status=200, body=json.dumps({"items": []}), headers=headers)
            return probes_mod.HttpResponse(status=200, body=json.dumps({"id": "ok"}), headers=headers)

        if token is None:
            return probes_mod.HttpResponse(status=401, body='{"code":"unauthorized"}', headers=headers)
        return probes_mod.HttpResponse(status=403, body='{"code":"forbidden"}', headers=headers)

    def run_denial_suite(self) -> dict[str, Any]:
        denial_mod = self._denial()
        mapping = denial_mod.build_http_sse_denial_mapping()
        subset = [entry for entry in mapping if entry.operation_id in {"getCollection", "getDocument", "createUpload"}]
        specs = denial_mod.build_denial_request_specs(subset, seed=self.seed, credentials=self.credentials)
        if self.skip_owner_control:
            specs = [spec for spec in specs if spec.scenario != "owner_control"]
        foreign_count = sum(1 for spec in specs if spec.scenario == "foreign")
        owner_count = sum(1 for spec in specs if spec.scenario == "owner_control")
        observations: list[Any] = []
        for spec in specs:
            response = self._http_request(
                method=spec.method,
                url="http://fake" + spec.path,
                token=spec.token,
                body=spec.body,
                path=spec.path,
                content_type=spec.content_type,
                multipart_body=spec.multipart_body,
                supplied_request_id=spec.supplied_request_id,
            )
            if response.status not in spec.expected_statuses and not self.accept_5xx:
                raise RuntimeError(
                    f"{spec.operation_id}/{spec.scenario} expected {sorted(spec.expected_statuses)} got {response.status}"
                )
            observations.append(
                denial_mod.DenialObservation(
                    operation_id=spec.operation_id,
                    row_id=spec.row_id,
                    scenario=spec.scenario,
                    expected_statuses=sorted(spec.expected_statuses),
                    actual_status=response.status,
                    body_sha256="fake",
                    request_id=response.headers.get("x-request-id"),
                    challenge_echo=None,
                    leaked_markers=[],
                )
            )
        denial_mod.validate_denial_observation_matrix(observations)
        return {"foreignCount": foreign_count, "ownerControlCount": owner_count}

    @staticmethod
    def assert_credentials_purged(path: Path) -> None:
        if path.exists():
            raise RuntimeError("credentials survived cleanup")
