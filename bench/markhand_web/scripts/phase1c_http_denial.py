#!/usr/bin/env python3
"""Black-box HTTP/SSE denial driver for Phase 1C deployed leakage proof."""

from __future__ import annotations

import hashlib
import json
import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Protocol

ROOT = Path(__file__).resolve().parents[3]
DEFAULT_MANIFEST = ROOT / "crates/server/tests/fixtures/multi-org-denial.manifest.json"
DEFAULT_GUARD_INVENTORY = ROOT / "crates/server/openapi/guard-inventory.json"
API_PREFIX = "/api/v1"

HttpRequestFn = Callable[..., Any]


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


@dataclass
class DenialObservation:
    operation_id: str
    row_id: str
    scenario: str
    expected_statuses: list[int]
    actual_status: int
    body_sha256: str
    request_id: str | None
    challenge_echo: str | None
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
                    "challengeEcho": item.challenge_echo,
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
        if item.get("evidenceRole") == "secondary":
            continue
        operation_id = item.get("operationId")
        test_name = item.get("testName")
        binary = item.get("binary")
        row_id = item.get("id")
        if not all(isinstance(value, str) and value.strip() for value in (operation_id, test_name, binary, row_id)):
            raise RuntimeError(f"manifest row missing executable HTTP/SSE fields: {row_id!r}")
        rows.append(
            ManifestRow(
                row_id=row_id,
                operation_id=operation_id,
                layer=layer,
                test_name=test_name,
                binary=binary,
            )
        )
    if not rows:
        raise RuntimeError("manifest has no executable HTTP/SSE rows")
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
    seen_primary_ops: set[str] = set()
    for row in rows:
        if row.row_id in seen_rows:
            raise RuntimeError(f"duplicate manifest row id {row.row_id}")
        seen_rows.add(row.row_id)
        guard_route = guard.get(row.operation_id)
        if guard_route is None:
            raise RuntimeError(f"missing guard inventory mapping for operationId {row.operation_id}")
        if row.operation_id in seen_primary_ops:
            raise RuntimeError(f"duplicate HTTP/SSE mapping for operationId {row.operation_id}")
        seen_primary_ops.add(row.operation_id)
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
            )
        )
    expected = len(rows)
    if len(mapping) != expected:
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


def _foreign_params(seed: Any) -> dict[str, str]:
    return {
        "orgId": seed.org_beta_id,
        "collectionId": seed.beta_collection_id,
        "documentId": seed.beta_document_id,
        "jobId": seed.beta_job_id,
        "userId": seed.beta_member_user_id,
        "sessionId": seed.beta_chat_session_id,
        "projectId": seed.beta_project_id,
        "versionId": seed.beta_version_id,
        "conflictId": seed.beta_conflict_id,
        "inviteId": seed.beta_invite_id,
        "capability": seed.beta_download_capability,
    }


def _body_for_operation(operation_id: str, seed: Any, *, foreign: bool) -> dict[str, Any] | None:
    collection_id = seed.beta_collection_id if foreign else seed.alpha_collection_id
    document_id = seed.beta_document_id if foreign else seed.alpha_document_id
    org_id = seed.org_beta_id if foreign else seed.org_alpha_id
    if operation_id in {"search", "ask"}:
        return {"query": f"phase1c-denial-{seed.challenge}", "collectionIds": [collection_id]}
    if operation_id == "askStream":
        return {"query": f"phase1c-denial-stream-{seed.challenge}", "collectionIds": [collection_id]}
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
        return {"token": seed.beta_invite_accept_token}
    if operation_id == "resolveCitation":
        return {"documentId": document_id, "versionId": seed.beta_version_id if foreign else seed.alpha_version_id}
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
        return {}
    if operation_id == "triageConflict":
        return {"resolution": "accept_local"}
    if operation_id == "updateCollection":
        return {"name": f"denial-updated-{seed.challenge[:8]}"}
    if operation_id == "updateProject":
        return {"name": f"denial-project-updated-{seed.challenge[:8]}"}
    if operation_id in {"revokeMemberInvite"}:
        return {}
    return None


def _uses_foreign_scope(operation_id: str, path_template: str) -> bool:
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
        return True
    return operation_id in {
        "search",
        "ask",
        "askStream",
        "assignCollectionProject",
        "resolveCitation",
        "switchOrg",
        "acceptMemberInvite",
    }


def build_denial_request_specs(
    mapping: list[DenialMappingEntry],
    *,
    seed: Any,
    credentials: Any,
) -> list[DenialRequestSpec]:
    specs: list[DenialRequestSpec] = []
    params = _foreign_params(seed)
    for entry in mapping:
        path = _substitute_path(entry.path_template, params)
        body = _body_for_operation(entry.operation_id, seed, foreign=True)
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
                    expected_statuses=frozenset({401, 403}),
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
                    expected_statuses=frozenset({401, 403, 404}),
                )
            )
    return specs


def _extract_request_id(body: str, headers: dict[str, str]) -> str | None:
    for key, value in headers.items():
        if key.lower() == "x-request-id" and value.strip():
            return value.strip()
    try:
        payload = json.loads(body)
    except json.JSONDecodeError:
        return None
    if isinstance(payload, dict):
        request_id = payload.get("requestId") or payload.get("request_id")
        if isinstance(request_id, str) and request_id.strip():
            return request_id.strip()
    return None


def _challenge_echo(body: str, headers: dict[str, str], challenge: str) -> str | None:
    header = headers.get("x-phase1c-challenge") or headers.get("X-Phase1c-Challenge")
    if isinstance(header, str) and header == challenge:
        return header
    if challenge in body:
        return None
    return None


def scan_marker_leakage(body: str, *, forbidden_markers: set[str]) -> list[str]:
    leaks: list[str] = []
    for marker in sorted(forbidden_markers):
        if marker and marker in body:
            leaks.append(marker)
    return leaks


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
    forbidden = {seed.marker_beta}
    for spec in specs:
        url = api_base.rstrip("/") + spec.path
        response = http_request(
            method=spec.method,
            url=url,
            token=spec.token,
            body=spec.body,
            path=spec.path,
        )
        leaked = scan_marker_leakage(response.body, forbidden_markers=forbidden)
        if response.status not in spec.expected_statuses:
            report.failures.append(
                f"{spec.operation_id}/{spec.scenario} expected {sorted(spec.expected_statuses)} got {response.status}"
            )
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
                request_id=_extract_request_id(response.body, response.headers),
                challenge_echo=_challenge_echo(response.body, response.headers, seed.challenge),
                leaked_markers=leaked,
            )
        )
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
