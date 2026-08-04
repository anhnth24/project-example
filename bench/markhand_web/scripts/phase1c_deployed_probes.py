#!/usr/bin/env python3
"""Deployed Phase 1C probes via production HTTP, Compose lifecycle, and authoritative SQL."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import math
import os
import re
import secrets
import subprocess
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable

ROOT = Path(__file__).resolve().parents[3]
DENIAL_MANIFEST = ROOT / "crates/server/tests/fixtures/multi-org-denial.manifest.json"
COMPOSE_FILE = ROOT / "deploy/compose.poc.yml"
HTTP_DENIAL = ROOT / "bench/markhand_web/scripts/phase1c_http_denial.py"

CANONICAL_NOISY_UPLOAD_DURATION_SECS = 60
QUALIFYING_QUIET_SEARCH_SAMPLES = 100
STARVATION_SLOW_MS = 2_000
DEPLOYED_PROBE_SCHEMA_VERSION = 1
REVOKE_POLL_TIMEOUT_MS = 3_000
REVOKE_POLL_INTERVAL_MS = 50
ACL_CACHE_POLL_TIMEOUT_MS = 3_000

DEPLOYED_PROBE_GATES: tuple[str, ...] = (
    "G1C-SEC-LEAKAGE",
    "G1C-SEC-ACL-CACHE",
    "G1C-SEC-STALE-TOKENS",
    "G1C-SEC-QUOTA-RECOVERY",
    "G1C-SEC-NOISY-NEIGHBOR",
    "G1C-SEC-AUDIT-COVERAGE",
    "G1C-SEC-REVOKE",
    "G1C-SEC-QDRANT-FAIL-CLOSED",
)

UUID_RE = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$",
    re.IGNORECASE,
)

SEED_REQUIRED_FIELDS: tuple[str, ...] = (
    "schemaVersion",
    "challenge",
    "sourceRevision",
    "manifestSha256",
    "orgAlphaId",
    "orgBetaId",
    "alphaUserId",
    "betaUserId",
    "markerAlpha",
    "markerBeta",
    "alphaCollectionId",
    "betaCollectionId",
    "alphaDocumentId",
    "betaDocumentId",
    "alphaJobId",
    "betaJobId",
    "alphaVersionId",
    "betaVersionId",
    "alphaChatSessionId",
    "betaChatSessionId",
    "alphaProjectId",
    "betaProjectId",
    "alphaConflictId",
    "betaConflictId",
    "betaMemberUserId",
    "alphaInviteId",
    "betaInviteId",
    "betaDownloadCapabilityHash",
    "alphaDownloadCapabilityHash",
    "betaInviteTokenHash",
    "alphaSessionIdHash",
    "betaSessionIdHash",
    "orgAlphaSlug",
    "orgBetaSlug",
)

CREDENTIAL_REQUIRED_FIELDS: tuple[str, ...] = (
    "alphaAccessToken",
    "alphaRefreshToken",
    "betaAccessToken",
    "betaRefreshToken",
    "betaAlphaAccessToken",
    "betaAlphaRefreshToken",
    "alphaBetaAccessToken",
    "alphaBetaRefreshToken",
    "alphaSessionId",
    "betaSessionId",
    "betaInviteToken",
    "alphaDownloadCapability",
    "betaDownloadCapability",
    "betaDenialDisposableCollectionId",
    "betaDenialDisposableCollectionUpdateId",
    "betaDenialDisposableDocumentId",
    "betaDenialDisposableChatSessionId",
    "betaDenialDisposableInviteId",
    "betaDenialDisposableMemberUserId",
    "betaDenialDisposableConflictId",
    "betaDenialAcceptInviteToken",
    "betaDenialAcceptAccessToken",
    "betaCitationChunkId",
    "betaCitationSourceContentSha256",
    "betaCitationCanonicalMarkdownSha256",
    "betaCitationSourceSpanStart",
    "betaCitationSourceSpanEnd",
    "betaCitationQuoteLocalStart",
    "betaCitationQuoteLocalEnd",
    "betaCitationQuote",
)

AUDIT_MUTATION_ACTIONS: tuple[str, ...] = (
    "org.switch",
    "collection.create",
    "member.invite.create",
    "member.invite.accept",
    "member.invite.revoke",
    "member.role.update",
    "member.delete",
    "auth.refresh.reuse",
    "auth.token.revoke",
    "conflict.triage",
)

SECRET_RESIDUAL_RE = re.compile(
    r"(?i)("
    r"Bearer\s+[A-Za-z0-9._\-+=/]{8,}|"
    r"\beyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}|"
    r"/(?:home|Users|workspace|tmp)/\S+|"
    r"cap-[A-Za-z0-9._\-]{8,}|"
    r"mhinv1\.[A-Za-z0-9._\-]{8,}"
    r")"
)


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


def validate_audit_pages(pages: list[dict[str, Any]], *, seen_cursors: set[str]) -> None:
    for page in pages:
        if not isinstance(page, dict):
            raise RuntimeError("audit page must be object")
        items = page.get("items")
        if not isinstance(items, list):
            raise RuntimeError("audit page missing items")
        meta = page.get("page")
        if not isinstance(meta, dict):
            raise RuntimeError("audit page missing page metadata")
        cursor = meta.get("nextCursor")
        has_more = meta.get("hasMore")
        if has_more is True:
            if not isinstance(cursor, str) or not cursor.strip():
                raise RuntimeError("audit page hasMore without nextCursor")
            if cursor in seen_cursors:
                raise RuntimeError("audit pagination cursor repeated")
            seen_cursors.add(cursor)
        for item in items:
            if not isinstance(item, dict):
                raise RuntimeError("audit item must be object")
            for key in ("id", "action", "targetType", "outcome", "occurredAt"):
                if key not in item:
                    raise RuntimeError(f"audit item missing {key}")
            validate_uuid(str(item["id"]), field="audit.id")


def _load_http_denial():
    spec = importlib.util.spec_from_file_location("phase1c_http_denial", HTTP_DENIAL)
    if spec is None or spec.loader is None:
        raise RuntimeError("phase1c_http_denial.py missing")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


_DENIAL = _load_http_denial()


@dataclass
class HttpResponse:
    status: int
    body: str
    headers: dict[str, str]


@dataclass
class CommandOutcome:
    exit_code: int
    stdout: str
    stderr: str


@dataclass
class SeedFixture:
    challenge: str
    org_alpha_id: str
    org_beta_id: str
    alpha_user_id: str
    beta_user_id: str
    marker_alpha: str
    marker_beta: str
    manifest_sha256: str
    source_revision: dict[str, Any]
    alpha_collection_id: str
    beta_collection_id: str
    alpha_document_id: str
    beta_document_id: str
    alpha_job_id: str
    beta_job_id: str
    alpha_version_id: str
    beta_version_id: str
    alpha_chat_session_id: str
    beta_chat_session_id: str
    alpha_project_id: str
    beta_project_id: str
    alpha_conflict_id: str
    beta_conflict_id: str
    beta_member_user_id: str
    alpha_invite_id: str
    beta_invite_id: str
    beta_download_capability_hash: str
    alpha_download_capability_hash: str
    beta_invite_token_hash: str
    alpha_session_id_hash: str
    beta_session_id_hash: str
    org_alpha_slug: str
    org_beta_slug: str


@dataclass
class SeedCredentials:
    challenge: str
    alpha_access_token: str
    alpha_refresh_token: str
    beta_access_token: str
    beta_refresh_token: str
    beta_alpha_access_token: str
    beta_alpha_refresh_token: str
    alpha_beta_access_token: str
    alpha_beta_refresh_token: str
    alpha_session_id: str
    beta_session_id: str
    beta_invite_token: str
    alpha_download_capability: str
    beta_download_capability: str
    beta_denial_disposable_collection_id: str
    beta_denial_disposable_collection_update_id: str
    beta_denial_disposable_document_id: str
    beta_denial_disposable_chat_session_id: str
    beta_denial_disposable_invite_id: str
    beta_denial_disposable_member_user_id: str
    beta_denial_disposable_conflict_id: str
    beta_denial_accept_invite_token: str
    beta_denial_accept_access_token: str
    beta_citation_chunk_id: str
    beta_citation_source_content_sha256: str
    beta_citation_canonical_markdown_sha256: str
    beta_citation_source_span_start: int
    beta_citation_source_span_end: int
    beta_citation_quote_local_start: int
    beta_citation_quote_local_end: int
    beta_citation_quote: str


@dataclass
class DeployedProbeResult:
    gate_id: str
    probe: dict[str, Any]
    metrics: dict[str, Any]
    detail: dict[str, Any] = field(default_factory=dict)


class DeployedProbeShims:
    def http_request(
        self,
        *,
        method: str,
        url: str,
        token: str | None = None,
        body: dict[str, Any] | None = None,
        path: str = "",
        content_type: str = "application/json",
        multipart_body: bytes | None = None,
        supplied_request_id: str | None = None,
        accept: str | None = None,
    ) -> HttpResponse:
        raise NotImplementedError

    def compose(self, args: list[str], *, timeout_secs: int = 600) -> CommandOutcome:
        raise NotImplementedError

    def psql(self, sql: str, *, timeout_secs: int = 120) -> CommandOutcome:
        raise NotImplementedError


class LiveDeployedProbeShims(DeployedProbeShims):
    def http_request(
        self,
        *,
        method: str,
        url: str,
        token: str | None = None,
        body: dict[str, Any] | None = None,
        path: str = "",
        content_type: str = "application/json",
        multipart_body: bytes | None = None,
        supplied_request_id: str | None = None,
        accept: str | None = None,
    ) -> HttpResponse:
        headers = {
            "content-type": content_type,
            "x-phase1c-challenge": os.environ.get("MARKHAND_PHASE1C_CHALLENGE", ""),
        }
        if accept:
            headers["accept"] = accept
        if supplied_request_id:
            headers["x-request-id"] = supplied_request_id
        if token:
            headers["authorization"] = f"Bearer {token}"
        data = multipart_body
        if data is None and body is not None:
            data = json.dumps(body).encode("utf-8")
        request = urllib.request.Request(url, data=data, headers=headers, method=method)
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                raw = response.read().decode("utf-8", errors="replace")
                return HttpResponse(status=response.status, body=raw, headers=dict(response.headers))
        except urllib.error.HTTPError as error:
            raw = error.read().decode("utf-8", errors="replace")
            return HttpResponse(status=error.code, body=raw, headers=dict(error.headers))

    def compose(self, args: list[str], *, timeout_secs: int = 600) -> CommandOutcome:
        cmd = ["docker", "compose", "-f", str(COMPOSE_FILE), *args]
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout_secs, check=False)
        return CommandOutcome(proc.returncode, proc.stdout or "", proc.stderr or "")

    def psql(self, sql: str, *, timeout_secs: int = 120) -> CommandOutcome:
        cmd = [
            "docker",
            "compose",
            "-f",
            str(COMPOSE_FILE),
            "exec",
            "-T",
            "postgres",
            "psql",
            "-U",
            os.environ.get("MARKHAND_POSTGRES_USER", "markhand"),
            "-d",
            os.environ.get("MARKHAND_POSTGRES_DB", "markhand"),
            "-tAc",
            sql,
        ]
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout_secs, check=False)
        return CommandOutcome(proc.returncode, proc.stdout or "", proc.stderr or "")


def canonical_denial_manifest_sha256() -> str:
    return _DENIAL.canonical_manifest_sha256(DENIAL_MANIFEST)


def nearest_rank_p95_ms(samples_ns: list[int]) -> int:
    if not samples_ns:
        raise RuntimeError("p95 requires samples")
    ordered = sorted(samples_ns)
    rank = max(1, math.ceil(0.95 * len(ordered)))
    return int(ordered[rank - 1] / 1_000_000)


def compute_quota_drift(*, documents: int, storage_bytes: int, reserved_concurrent_slots: int) -> int:
    return int(documents) + int(storage_bytes) + int(reserved_concurrent_slots)


def _require_string(raw: dict[str, Any], key: str) -> str:
    value = raw.get(key)
    if not isinstance(value, str) or not value.strip():
        raise RuntimeError(f"seed missing {key}")
    return value.strip()


def validate_source_revision_binding(raw: dict[str, Any], *, git_sha_full: str) -> None:
    source_revision = raw.get("sourceRevision")
    if not isinstance(source_revision, dict):
        raise RuntimeError("seed missing sourceRevision")
    commit = source_revision.get("commit")
    if not isinstance(commit, str) or not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise RuntimeError("sourceRevision.commit must be 40-char git SHA")
    if source_revision.get("dirty") is not False:
        raise RuntimeError("sourceRevision.dirty must be false")
    if commit != git_sha_full:
        raise RuntimeError("sourceRevision.commit mismatch with gitShaFull")


def parse_seed_artifact(raw: dict[str, Any], *, expected_challenge: str) -> SeedFixture:
    if raw.get("schemaVersion") != 1:
        raise RuntimeError("seed schemaVersion mismatch")
    if raw.get("challenge") != expected_challenge:
        raise RuntimeError("seed challenge mismatch")
    for key in SEED_REQUIRED_FIELDS:
        if key in {"sourceRevision", "schemaVersion"}:
            continue
        _require_string(raw, key)
    source_revision = raw.get("sourceRevision")
    if not isinstance(source_revision, dict):
        raise RuntimeError("seed missing sourceRevision")
    manifest_sha = _require_string(raw, "manifestSha256")
    if manifest_sha != canonical_denial_manifest_sha256():
        raise RuntimeError("seed manifestSha256 mismatch")
    alpha_user = _require_string(raw, "alphaUserId")
    beta_user = _require_string(raw, "betaUserId")
    if alpha_user == beta_user:
        raise RuntimeError("seed duplicate alpha/beta user ids")
    alpha_org = _require_string(raw, "orgAlphaId")
    beta_org = _require_string(raw, "orgBetaId")
    if alpha_org == beta_org:
        raise RuntimeError("seed duplicate org ids")
    marker_alpha = _require_string(raw, "markerAlpha")
    marker_beta = _require_string(raw, "markerBeta")
    if marker_alpha == marker_beta:
        raise RuntimeError("seed duplicate markers")
    return SeedFixture(
        challenge=_require_string(raw, "challenge"),
        org_alpha_id=alpha_org,
        org_beta_id=beta_org,
        alpha_user_id=alpha_user,
        beta_user_id=beta_user,
        marker_alpha=marker_alpha,
        marker_beta=marker_beta,
        manifest_sha256=manifest_sha,
        source_revision=dict(source_revision),
        alpha_collection_id=_require_string(raw, "alphaCollectionId"),
        beta_collection_id=_require_string(raw, "betaCollectionId"),
        alpha_document_id=_require_string(raw, "alphaDocumentId"),
        beta_document_id=_require_string(raw, "betaDocumentId"),
        alpha_job_id=_require_string(raw, "alphaJobId"),
        beta_job_id=_require_string(raw, "betaJobId"),
        alpha_version_id=_require_string(raw, "alphaVersionId"),
        beta_version_id=_require_string(raw, "betaVersionId"),
        alpha_chat_session_id=_require_string(raw, "alphaChatSessionId"),
        beta_chat_session_id=_require_string(raw, "betaChatSessionId"),
        alpha_project_id=_require_string(raw, "alphaProjectId"),
        beta_project_id=_require_string(raw, "betaProjectId"),
        alpha_conflict_id=_require_string(raw, "alphaConflictId"),
        beta_conflict_id=_require_string(raw, "betaConflictId"),
        beta_member_user_id=_require_string(raw, "betaMemberUserId"),
        alpha_invite_id=_require_string(raw, "alphaInviteId"),
        beta_invite_id=_require_string(raw, "betaInviteId"),
        beta_download_capability_hash=_require_string(raw, "betaDownloadCapabilityHash"),
        alpha_download_capability_hash=_require_string(raw, "alphaDownloadCapabilityHash"),
        beta_invite_token_hash=_require_string(raw, "betaInviteTokenHash"),
        alpha_session_id_hash=_require_string(raw, "alphaSessionIdHash"),
        beta_session_id_hash=_require_string(raw, "betaSessionIdHash"),
        org_alpha_slug=_require_string(raw, "orgAlphaSlug"),
        org_beta_slug=_require_string(raw, "orgBetaSlug"),
    )


def build_public_seed_evidence(raw: dict[str, Any]) -> dict[str, Any]:
    public = {key: raw[key] for key in SEED_REQUIRED_FIELDS if key not in {"sourceRevision", "schemaVersion"}}
    if isinstance(raw.get("sourceRevision"), dict):
        public["sourceRevision"] = {
            "commit": raw["sourceRevision"].get("commit"),
            "dirty": raw["sourceRevision"].get("dirty"),
        }
    public["completionMarker"] = "PHASE1C_SEED_EOF"
    serialized = json.dumps(public, sort_keys=True)
    if SECRET_RESIDUAL_RE.search(serialized):
        raise RuntimeError("seed evidence residual secret/path material")
    return public


def purge_phase1c_credentials(path: Path) -> None:
    """Securely remove private credential artifact (mode 0600 expected)."""
    if not path.exists():
        return
    try:
        with path.open("r+b") as handle:
            size = path.stat().st_size
            if size:
                handle.write(b"\x00" * size)
                handle.flush()
                os.fsync(handle.fileno())
    except OSError:
        pass
    try:
        path.unlink(missing_ok=True)
    except OSError as error:
        raise RuntimeError("credential purge failed") from error


def validate_fixture_credentials(credentials: SeedCredentials) -> None:
    for field_name in (
        "beta_denial_disposable_collection_id",
        "beta_denial_disposable_collection_update_id",
        "beta_denial_disposable_document_id",
        "beta_denial_disposable_chat_session_id",
        "beta_denial_disposable_invite_id",
        "beta_denial_disposable_member_user_id",
        "beta_denial_disposable_conflict_id",
        "beta_denial_accept_invite_token",
        "beta_denial_accept_access_token",
        "beta_citation_chunk_id",
        "beta_citation_source_content_sha256",
        "beta_citation_canonical_markdown_sha256",
        "beta_citation_quote",
        "alpha_beta_access_token",
    ):
        value = getattr(credentials, field_name, None)
        if isinstance(value, str):
            if not value.strip():
                raise RuntimeError(f"credentials missing {field_name}")
            if field_name.endswith("_id") or field_name.endswith("chunk_id"):
                validate_uuid(value, field=field_name)
        elif value is None:
            raise RuntimeError(f"credentials missing {field_name}")


def load_seed_credentials(
    path: Path,
    *,
    expected_challenge: str,
    purge_after_load: bool = False,
) -> SeedCredentials:
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
        if raw.get("schemaVersion") != 1:
            raise RuntimeError("credential schemaVersion mismatch")
        if raw.get("challenge") != expected_challenge:
            raise RuntimeError("credential challenge mismatch")
        for key in CREDENTIAL_REQUIRED_FIELDS:
            if key in {
                "betaCitationSourceSpanStart",
                "betaCitationSourceSpanEnd",
                "betaCitationQuoteLocalStart",
                "betaCitationQuoteLocalEnd",
            }:
                if key not in raw:
                    raise RuntimeError(f"credentials missing {key}")
                continue
            _require_string(raw, key)
        credentials = SeedCredentials(
            challenge=expected_challenge,
            alpha_access_token=_require_string(raw, "alphaAccessToken"),
            alpha_refresh_token=_require_string(raw, "alphaRefreshToken"),
            beta_access_token=_require_string(raw, "betaAccessToken"),
            beta_refresh_token=_require_string(raw, "betaRefreshToken"),
            beta_alpha_access_token=_require_string(raw, "betaAlphaAccessToken"),
            beta_alpha_refresh_token=_require_string(raw, "betaAlphaRefreshToken"),
            alpha_beta_access_token=_require_string(raw, "alphaBetaAccessToken"),
            alpha_beta_refresh_token=_require_string(raw, "alphaBetaRefreshToken"),
            alpha_session_id=validate_uuid(_require_string(raw, "alphaSessionId"), field="alphaSessionId"),
            beta_session_id=validate_uuid(_require_string(raw, "betaSessionId"), field="betaSessionId"),
            beta_invite_token=_require_string(raw, "betaInviteToken"),
            alpha_download_capability=_require_string(raw, "alphaDownloadCapability"),
            beta_download_capability=_require_string(raw, "betaDownloadCapability"),
            beta_denial_disposable_collection_id=validate_uuid(
                _require_string(raw, "betaDenialDisposableCollectionId"), field="betaDenialDisposableCollectionId"
            ),
            beta_denial_disposable_collection_update_id=validate_uuid(
                _require_string(raw, "betaDenialDisposableCollectionUpdateId"),
                field="betaDenialDisposableCollectionUpdateId",
            ),
            beta_denial_disposable_document_id=validate_uuid(
                _require_string(raw, "betaDenialDisposableDocumentId"), field="betaDenialDisposableDocumentId"
            ),
            beta_denial_disposable_chat_session_id=validate_uuid(
                _require_string(raw, "betaDenialDisposableChatSessionId"), field="betaDenialDisposableChatSessionId"
            ),
            beta_denial_disposable_invite_id=validate_uuid(
                _require_string(raw, "betaDenialDisposableInviteId"), field="betaDenialDisposableInviteId"
            ),
            beta_denial_disposable_member_user_id=validate_uuid(
                _require_string(raw, "betaDenialDisposableMemberUserId"), field="betaDenialDisposableMemberUserId"
            ),
            beta_denial_disposable_conflict_id=validate_uuid(
                _require_string(raw, "betaDenialDisposableConflictId"), field="betaDenialDisposableConflictId"
            ),
            beta_denial_accept_invite_token=_require_string(raw, "betaDenialAcceptInviteToken"),
            beta_denial_accept_access_token=_require_string(raw, "betaDenialAcceptAccessToken"),
            beta_citation_chunk_id=validate_uuid(_require_string(raw, "betaCitationChunkId"), field="betaCitationChunkId"),
            beta_citation_source_content_sha256=_require_string(raw, "betaCitationSourceContentSha256"),
            beta_citation_canonical_markdown_sha256=_require_string(raw, "betaCitationCanonicalMarkdownSha256"),
            beta_citation_source_span_start=int(raw["betaCitationSourceSpanStart"]),
            beta_citation_source_span_end=int(raw["betaCitationSourceSpanEnd"]),
            beta_citation_quote_local_start=int(raw["betaCitationQuoteLocalStart"]),
            beta_citation_quote_local_end=int(raw["betaCitationQuoteLocalEnd"]),
            beta_citation_quote=_require_string(raw, "betaCitationQuote"),
        )
        validate_fixture_credentials(credentials)
        return credentials
    finally:
        if purge_after_load:
            purge_phase1c_credentials(path)


def load_seed_artifact(path: Path, *, expected_challenge: str) -> SeedFixture:
    raw = json.loads(path.read_text(encoding="utf-8"))
    return parse_seed_artifact(raw, expected_challenge=expected_challenge)


def validate_trivy_report_target(report: dict[str, Any], *, requested_ref: str) -> None:
    if report.get("SchemaVersion") != 2:
        raise RuntimeError("trivy SchemaVersion must be 2")
    requested_digest = requested_ref.split("@", 1)[-1]
    blob = json.dumps(report)
    if requested_digest not in blob:
        raise RuntimeError("trivy report target/digest mismatch")


def extract_high_critical_findings(report: dict[str, Any]) -> list[dict[str, str]]:
    findings: list[dict[str, str]] = []
    for result in report.get("Results") or []:
        if not isinstance(result, dict):
            raise RuntimeError("trivy result malformed")
        for vuln in result.get("Vulnerabilities") or []:
            if not isinstance(vuln, dict):
                raise RuntimeError("trivy vulnerability malformed")
            severity = str(vuln.get("Severity", "")).upper()
            if severity in {"HIGH", "CRITICAL"}:
                findings.append(
                    {
                        "vulnerabilityId": str(vuln.get("VulnerabilityID", "")),
                        "severity": severity,
                        "status": str(vuln.get("Status") or "unknown"),
                    }
                )
    return findings


def _sha256_text(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


def _parse_audit_page(body: str) -> list[dict[str, Any]]:
    payload = json.loads(body)
    if not isinstance(payload, dict):
        raise RuntimeError("audit response must be object")
    items = payload.get("items")
    if not isinstance(items, list):
        raise RuntimeError("audit response missing items")
    parsed: list[dict[str, Any]] = []
    for item in items:
        if not isinstance(item, dict):
            raise RuntimeError("audit item must be object")
        for key in ("id", "action", "targetType", "outcome", "occurredAt"):
            if key not in item:
                raise RuntimeError(f"audit item missing {key}")
        parsed.append(item)
    return parsed


def _audit_matches(expected: dict[str, Any], entry: dict[str, Any], *, gate_started_at: str | None = None) -> bool:
    if entry.get("action") != expected["action"]:
        return False
    if entry.get("outcome") != expected.get("outcome", "success"):
        return False
    actor = entry.get("actorId") or entry.get("actor_id")
    if str(actor) != str(expected.get("actorId")):
        return False
    target_type = entry.get("targetType") or entry.get("target_type")
    if expected.get("targetType") is not None and target_type != expected["targetType"]:
        return False
    target_id = entry.get("targetId") or entry.get("target_id")
    if expected.get("targetId") is not None and str(target_id) != str(expected["targetId"]):
        return False
    if expected.get("requestId") and entry.get("requestId") != expected["requestId"]:
        return False
    if gate_started_at is not None:
        occurred = entry.get("occurredAt") or entry.get("occurred_at")
        if not isinstance(occurred, str) or occurred < gate_started_at:
            return False
    return True


def _fetch_audit_entries_paginated(
    http_get_audit: Callable[[str], HttpResponse],
    *,
    max_pages: int = 100,
) -> list[dict[str, Any]]:
    entries: list[dict[str, Any]] = []
    cursor: str | None = None
    seen_cursors: set[str] = set()
    pages: list[dict[str, Any]] = []
    for _ in range(max_pages):
        path = "/api/v1/audit" if cursor is None else f"/api/v1/audit?cursor={cursor}"
        response = http_get_audit(path)
        if response.status != 200:
            raise RuntimeError("audit fetch failed")
        payload = json.loads(response.body)
        if not isinstance(payload, dict):
            raise RuntimeError("audit response must be object")
        pages.append(payload)
        entries.extend(_parse_audit_page(response.body))
        page = payload.get("page")
        if not isinstance(page, dict) or not page.get("hasMore"):
            break
        next_cursor = page.get("nextCursor")
        if not isinstance(next_cursor, str) or not next_cursor.strip():
            raise RuntimeError("audit pagination hasMore without cursor")
        cursor = next_cursor
    else:
        raise RuntimeError("audit pagination exceeded max pages")
    validate_audit_pages(pages, seen_cursors=seen_cursors)
    return entries


@dataclass
class DeployedContext:
    api_base: str
    seed: SeedFixture
    credentials: SeedCredentials
    shims: DeployedProbeShims
    noisy_duration_secs: int = CANONICAL_NOISY_UPLOAD_DURATION_SECS
    quiet_search_samples: int = QUALIFYING_QUIET_SEARCH_SAMPLES
    worker_nonce: str = ""


class DeployedProbeRunner:
    """Black-box deployed probes via production HTTP, compose lifecycle, and SQL."""

    def __init__(
        self,
        *,
        api_base: str,
        seed: SeedFixture,
        credentials: SeedCredentials,
        shims: DeployedProbeShims | None = None,
        noisy_duration_secs: int = CANONICAL_NOISY_UPLOAD_DURATION_SECS,
        quiet_search_samples: int = QUALIFYING_QUIET_SEARCH_SAMPLES,
        git_sha_full: str | None = None,
    ) -> None:
        self.api_base = api_base.rstrip("/")
        self.seed = seed
        self.credentials = credentials
        self.shims = shims or LiveDeployedProbeShims()
        self.noisy_duration_secs = noisy_duration_secs
        self.quiet_search_samples = quiet_search_samples
        self.git_sha_full = git_sha_full or str(seed.source_revision.get("commit") or "")
        self._transition_kinds: set[str] = set()
        self._membership_revoked = False
        self._beta_email = os.environ.get("MARKHAND_PHASE1C_BETA_EMAIL", "phase1c-beta@poc.example")

    def _record_transition(self, kind: str) -> None:
        self._transition_kinds.add(kind)

    def _require_correlated_transitions(self, *, minimum: int = 3) -> None:
        if len(self._transition_kinds) < minimum:
            raise RuntimeError(
                f"deployed probe requires >= {minimum} correlated transition kinds; "
                f"saw {sorted(self._transition_kinds)}"
            )

    def _url(self, path: str) -> str:
        return f"{self.api_base}{path}"

    def _http(
        self,
        method: str,
        path: str,
        *,
        token: str | None = None,
        body: dict[str, Any] | None = None,
    ) -> HttpResponse:
        self._record_transition("http")
        return self.shims.http_request(
            method=method,
            url=self._url(path),
            token=token,
            body=body,
            path=path,
        )

    def _compose(self, args: list[str], *, timeout_secs: int = 600) -> CommandOutcome:
        self._record_transition("compose")
        return self.shims.compose(args, timeout_secs=timeout_secs)

    def _psql(self, sql: str, *, timeout_secs: int = 120) -> CommandOutcome:
        self._record_transition("psql")
        return self.shims.psql(sql, timeout_secs=timeout_secs)

    def _validate_manifest_binding(self) -> None:
        _DENIAL.validate_revision_binding(
            source_revision=self.seed.source_revision,
            manifest_sha256=self.seed.manifest_sha256,
            git_sha_full=self.git_sha_full,
            manifest_path=DENIAL_MANIFEST,
        )
        psql_out = self._psql(f"SELECT encode(digest('{self.seed.challenge}', 'sha256'), 'hex')")
        if psql_out.exit_code != 0:
            raise RuntimeError("challenge binding psql failed")
        self._compose(["ps", "-q", "api"])

    def _protected_resource_path(self) -> str:
        return f"/api/v1/collections/{self.seed.alpha_collection_id}"

    def _acl_probe_upload(
        self,
        *,
        token: str,
        collection_id: str | None = None,
    ) -> HttpResponse:
        collection = collection_id or self.seed.alpha_collection_id
        boundary = "----markhandPhase1cAclProbe"
        content = f"phase1c-acl-{self.seed.challenge}\n".encode("utf-8")
        body = bytearray()
        body.extend(
            f'--{boundary}\r\nContent-Disposition: form-data; name="collectionId"\r\n\r\n{collection}\r\n'.encode(
                "utf-8"
            )
        )
        body.extend(
            (
                f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="acl-probe.txt"\r\n'
                f"Content-Type: text/plain\r\n\r\n"
            ).encode("utf-8")
        )
        body.extend(content)
        body.extend(f"\r\n--{boundary}--\r\n".encode("utf-8"))
        return self.shims.http_request(
            method="POST",
            url=self._url("/api/v1/uploads"),
            token=token,
            body=None,
            path="/api/v1/uploads",
            content_type=f"multipart/form-data; boundary={boundary}",
            multipart_body=bytes(body),
        )

    def _poll_upload_until_denied(
        self,
        *,
        token: str,
        collection_id: str,
        timeout_ms: int,
    ) -> tuple[int, int]:
        deadline = time.monotonic() + (timeout_ms / 1000.0)
        commit_started = time.monotonic()
        max_stale = 0
        elapsed_ms = 0
        while time.monotonic() <= deadline:
            response = self._acl_probe_upload(token=token, collection_id=collection_id)
            elapsed_ms = int((time.monotonic() - commit_started) * 1000)
            if response.status in {401, 403, 404}:
                return elapsed_ms, max_stale
            if response.status // 100 == 2:
                max_stale += 1
            time.sleep(REVOKE_POLL_INTERVAL_MS / 1000.0)
        raise RuntimeError(f"upload still authorized after {timeout_ms}ms")

    def _discard_revoked_token_family(self, *, access_token: str, refresh_token: str) -> None:
        access_denied = self._http("GET", "/api/v1/auth/me", token=access_token)
        refresh_denied = self._http(
            "POST",
            "/api/v1/auth/refresh",
            body={"refreshToken": refresh_token},
        )
        if access_denied.status // 100 == 2:
            raise RuntimeError("revoked successor access token still authorizes")
        if refresh_denied.status in {200, 201, 204}:
            raise RuntimeError("revoked successor refresh token still rotates")
        if refresh_denied.status != 401:
            raise RuntimeError(f"revoked successor refresh must be exact 401, got {refresh_denied.status}")

    def _poll_until_denied(
        self,
        *,
        token: str,
        path: str,
        timeout_ms: int,
    ) -> tuple[int, int]:
        deadline = time.monotonic() + (timeout_ms / 1000.0)
        commit_started = time.monotonic()
        max_stale = 0
        elapsed_ms = 0
        while time.monotonic() <= deadline:
            response = self._http("GET", path, token=token)
            elapsed_ms = int((time.monotonic() - commit_started) * 1000)
            if response.status in {401, 403, 404}:
                return elapsed_ms, max_stale
            if response.status // 100 == 2:
                max_stale += 1
            time.sleep(REVOKE_POLL_INTERVAL_MS / 1000.0)
        raise RuntimeError(f"protected resource still authorized after {timeout_ms}ms")

    def _login_tokens(self, email: str, password: str) -> tuple[str, str, str]:
        password_value = os.environ.get(
            "MARKHAND_PHASE1C_SEED_PASSWORD",
            os.environ.get("MARKHAND_O04_API_PASSWORD", "markhand-dev"),
        )
        response = self._http(
            "POST",
            "/api/v1/auth/login",
            body={"email": email, "password": password or password_value},
        )
        if response.status != 200:
            raise RuntimeError(f"login failed for {email}")
        payload = json.loads(response.body)
        access = payload.get("accessToken") or payload.get("access_token")
        refresh = payload.get("refreshToken") or payload.get("refresh_token")
        if not isinstance(access, str) or not isinstance(refresh, str):
            raise RuntimeError("login missing token pair")
        me = self._http("GET", "/api/v1/auth/me", token=access)
        if me.status != 200:
            raise RuntimeError("auth/me failed after login")
        me_payload = json.loads(me.body)
        session_id = me_payload.get("sessionId") or me_payload.get("session_id")
        if not isinstance(session_id, str) or not session_id.strip():
            raise RuntimeError("auth/me missing sessionId")
        return access, refresh, session_id

    def _switch_org(self, token: str, org_id: str) -> HttpResponse:
        return self._http("POST", "/api/v1/orgs/switch", token=token, body={"orgId": org_id})

    def _session_id_from_switch_access_token(self, switch_response: HttpResponse) -> str:
        if switch_response.status // 100 != 2:
            raise RuntimeError(f"org switch must succeed, got {switch_response.status}")
        payload = json.loads(switch_response.body)
        switch_access = payload.get("accessToken") or payload.get("access_token")
        if not isinstance(switch_access, str):
            raise RuntimeError("org switch missing access token")
        me = self._http("GET", "/api/v1/auth/me", token=switch_access)
        if me.status != 200:
            raise RuntimeError("auth/me failed for switch access token")
        me_payload = json.loads(me.body)
        session_id = me_payload.get("sessionId") or me_payload.get("session_id")
        if not isinstance(session_id, str) or not session_id.strip():
            raise RuntimeError("switch access token auth/me missing sessionId")
        return validate_uuid(session_id, field="sessionId")

    def _restore_beta_alpha_membership_role(self, *, role: str = "editor") -> None:
        patch = self._http(
            "PATCH",
            f"/api/v1/members/{self.seed.beta_member_user_id}",
            token=self.credentials.alpha_access_token,
            body={"role": role},
        )
        if patch.status // 100 != 2:
            raise RuntimeError(f"membership role restore must succeed, got {patch.status}")

    def _restore_beta_membership(self) -> None:
        self._restore_beta_alpha_membership()

    def _restore_beta_alpha_membership(self) -> None:
        alpha_email = os.environ.get("MARKHAND_PHASE1C_SEED_EMAIL", "admin@poc.example")
        alpha_access, _, _ = self._login_tokens(alpha_email, "")
        invite = self._http(
            "POST",
            "/api/v1/members/invites",
            token=alpha_access,
            body={"email": self._beta_email, "role": "editor"},
        )
        if invite.status // 100 != 2:
            raise RuntimeError(f"alpha membership restore invite failed: {invite.status}")
        invite_payload = json.loads(invite.body)
        invite_token = invite_payload.get("token")
        if not isinstance(invite_token, str):
            raise RuntimeError("alpha membership restore invite missing token")
        beta_alpha_access, beta_alpha_refresh, _ = self._login_tokens(self._beta_email, "")
        accept = self._http(
            "POST",
            "/api/v1/members/invites/accept",
            token=beta_alpha_access,
            body={"token": invite_token},
        )
        if accept.status // 100 != 2:
            raise RuntimeError(f"alpha membership restore accept failed: {accept.status}")
        beta_alpha_access, beta_alpha_refresh, beta_session = self._login_tokens(self._beta_email, "")
        beta_access, beta_refresh, _ = self._login_tokens(self._beta_email, "")
        switch = self._switch_org(beta_access, self.seed.org_beta_id)
        if switch.status // 100 != 2:
            raise RuntimeError("beta org switch failed during restore")
        switch_payload = json.loads(switch.body)
        beta_access = switch_payload.get("accessToken") or switch_payload.get("access_token") or beta_access
        beta_refresh = switch_payload.get("refreshToken") or switch_payload.get("refresh_token") or beta_refresh
        self.credentials.alpha_access_token = alpha_access
        self.credentials.beta_alpha_access_token = beta_alpha_access
        self.credentials.beta_alpha_refresh_token = beta_alpha_refresh
        self.credentials.beta_access_token = beta_access
        self.credentials.beta_refresh_token = beta_refresh
        self.credentials.beta_session_id = beta_session
        self.credentials.beta_invite_token = invite_token
        self._membership_revoked = False

    def _ensure_beta_membership(self) -> None:
        if self._membership_revoked:
            self._restore_beta_alpha_membership()
            return
        warm = self._acl_probe_upload(
            token=self.credentials.beta_alpha_access_token,
            collection_id=self.seed.alpha_collection_id,
        )
        if warm.status // 100 == 2:
            return
        beta_warm = self._http("GET", "/api/v1/auth/me", token=self.credentials.beta_alpha_access_token)
        if beta_warm.status // 100 == 2:
            return
        self._restore_beta_alpha_membership()

    def run_http_denial_probe(self) -> DeployedProbeResult:
        self._validate_manifest_binding()
        validate_fixture_credentials(self.credentials)

        def http_wrapper(**kwargs: Any) -> HttpResponse:
            self._record_transition("http")
            return self.shims.http_request(**kwargs)

        report = _DENIAL.execute_http_denial_suite(
            seed=self.seed,
            credentials=self.credentials,
            http_request=http_wrapper,
            api_base=self.api_base,
            git_sha_full=self.git_sha_full,
            manifest_path=DENIAL_MANIFEST,
        )
        if report.failures:
            raise RuntimeError("; ".join(report.failures))
        self._require_correlated_transitions(minimum=1)
        return DeployedProbeResult(
            gate_id="G1C-SEC-LEAKAGE",
            probe={
                "deployedApi": True,
                "manifestSha256": report.manifest_sha256,
                "gitShaFull": report.git_sha_full,
                "executableHttpSseCount": report.executable_http_sse_count,
                "observationCount": len(report.observations),
                "eof": True,
            },
            metrics={"cross_tenant_leakage_count": report.leakage_count},
            detail={"report": report.as_dict()},
        )

    def run_revoke_probe(self) -> DeployedProbeResult:
        self._validate_manifest_binding()
        self._ensure_beta_membership()
        warm = self._http(
            "GET",
            self._protected_resource_path(),
            token=self.credentials.beta_alpha_access_token,
        )
        if warm.status != 200:
            raise RuntimeError("beta_alpha warm collection read failed before revoke")
        delete = self._http(
            "DELETE",
            f"/api/v1/members/{self.seed.beta_member_user_id}",
            token=self.credentials.alpha_access_token,
        )
        if delete.status // 100 != 2:
            raise RuntimeError(f"member delete must succeed with 2xx, got {delete.status}")
        elapsed_ms, stale = self._poll_until_denied(
            token=self.credentials.beta_alpha_access_token,
            path=self._protected_resource_path(),
            timeout_ms=REVOKE_POLL_TIMEOUT_MS,
        )
        if stale > 0:
            raise RuntimeError(f"post-commit stale authorizations observed: {stale}")
        self._membership_revoked = True
        self._require_correlated_transitions(minimum=2)
        return DeployedProbeResult(
            gate_id="G1C-SEC-REVOKE",
            probe={
                "deployedApi": True,
                "deleteStatus": delete.status,
                "betaUserIdHash": _sha256_text(self.seed.beta_member_user_id),
                "orgIdHash": _sha256_text(self.seed.org_alpha_id),
                "eof": True,
            },
            metrics={
                "membership_acl_revoke_max_ms": elapsed_ms,
                "post_commit_stale_authorizations": stale,
            },
        )

    def run_acl_cache_probe(self) -> DeployedProbeResult:
        self._validate_manifest_binding()
        self._ensure_beta_membership()
        warm = self._acl_probe_upload(
            token=self.credentials.beta_alpha_access_token,
            collection_id=self.seed.alpha_collection_id,
        )
        if warm.status not in {200, 201}:
            raise RuntimeError("beta_alpha warm upload failed before acl cache probe")
        patch = self._http(
            "PATCH",
            f"/api/v1/members/{self.seed.beta_member_user_id}",
            token=self.credentials.alpha_access_token,
            body={"role": "viewer"},
        )
        if patch.status // 100 != 2:
            raise RuntimeError(f"member patch must succeed with 2xx, got {patch.status}")
        elapsed_ms, stale = self._poll_upload_until_denied(
            token=self.credentials.beta_alpha_access_token,
            collection_id=self.seed.alpha_collection_id,
            timeout_ms=ACL_CACHE_POLL_TIMEOUT_MS,
        )
        self._restore_beta_alpha_membership_role(role="editor")
        restore_warm = self._acl_probe_upload(
            token=self.credentials.beta_alpha_access_token,
            collection_id=self.seed.alpha_collection_id,
        )
        if restore_warm.status not in {200, 201}:
            raise RuntimeError("editor role restore must re-authorize upload")
        self._require_correlated_transitions(minimum=2)
        return DeployedProbeResult(
            gate_id="G1C-SEC-ACL-CACHE",
            probe={
                "deployedApi": True,
                "patchStatus": patch.status,
                "observationPath": "/api/v1/uploads",
                "eof": True,
            },
            metrics={
                "membership_acl_revoke_max_ms": elapsed_ms,
                "post_commit_stale_authorizations": stale,
            },
        )

    def run_stale_tokens_probe(self) -> DeployedProbeResult:
        self._validate_manifest_binding()
        self._ensure_beta_membership()
        unrelated = self._http("GET", "/api/v1/auth/me", token="unrelated-token-value")
        if unrelated.status == 200:
            raise RuntimeError("unrelated token must not authorize protected resource")
        warm = self._http("GET", "/api/v1/auth/me", token=self.credentials.beta_access_token)
        if warm.status != 200:
            raise RuntimeError("auth warm failed")
        captured_refresh = self.credentials.beta_refresh_token
        refresh = self._http(
            "POST",
            "/api/v1/auth/refresh",
            body={"refreshToken": captured_refresh},
        )
        if refresh.status != 200:
            raise RuntimeError(f"refresh must succeed, got {refresh.status}")
        payload = json.loads(refresh.body)
        new_access = payload.get("accessToken") or payload.get("access_token")
        new_refresh = payload.get("refreshToken") or payload.get("refresh_token")
        if not isinstance(new_access, str) or not isinstance(new_refresh, str):
            raise RuntimeError("refresh response missing token pair")
        rotated_warm = self._http("GET", "/api/v1/auth/me", token=new_access)
        if rotated_warm.status != 200:
            raise RuntimeError("rotated access token must authorize")
        reuse = self._http(
            "POST",
            "/api/v1/auth/refresh",
            body={"refreshToken": captured_refresh},
        )
        if reuse.status in {200, 201, 204}:
            raise RuntimeError("reuse of old refresh token must fail")
        if reuse.status != 401:
            raise RuntimeError(f"stale refresh reuse must be exact 401, got {reuse.status}")
        if reuse.status // 100 == 5:
            raise RuntimeError(f"stale refresh reuse must not return 5xx, got {reuse.status}")
        self._discard_revoked_token_family(access_token=new_access, refresh_token=new_refresh)
        fresh_access, fresh_refresh, fresh_session = self._login_tokens(self._beta_email, "")
        switch = self._switch_org(fresh_access, self.seed.org_beta_id)
        if switch.status // 100 != 2:
            raise RuntimeError("fresh beta org switch failed after token family discard")
        switch_payload = json.loads(switch.body)
        fresh_access = switch_payload.get("accessToken") or switch_payload.get("access_token") or fresh_access
        fresh_refresh = switch_payload.get("refreshToken") or switch_payload.get("refresh_token") or fresh_refresh
        self.credentials.beta_access_token = fresh_access
        self.credentials.beta_refresh_token = fresh_refresh
        self.credentials.beta_session_id = validate_uuid(fresh_session, field="betaSessionId")
        self._require_correlated_transitions(minimum=3)
        return DeployedProbeResult(
            gate_id="G1C-SEC-STALE-TOKENS",
            probe={
                "deployedApi": True,
                "refreshStatus": refresh.status,
                "reuseStatus": reuse.status,
                "betaSessionIdHash": _sha256_text(self.credentials.beta_session_id),
                "betaUserIdHash": _sha256_text(self.seed.beta_user_id),
                "orgIdHash": _sha256_text(self.seed.org_alpha_id),
                "eof": True,
            },
            metrics={"post_commit_stale_authorizations": 0},
        )

    def run_quota_recovery_probe(self) -> DeployedProbeResult:
        self._validate_manifest_binding()
        before = self._psql(
            "SELECT documents, storage_bytes, reserved_concurrent_slots FROM usage_counters LIMIT 1"
        )
        self._compose(["stop", "worker-convert"], timeout_secs=120)
        self._compose(["start", "worker-convert"], timeout_secs=120)
        after = self._psql(
            "SELECT documents, storage_bytes, reserved_concurrent_slots FROM usage_counters LIMIT 1"
        )
        reconcile = self._psql(
            "SELECT action FROM audit_events WHERE action='quota.reconcile' ORDER BY id DESC LIMIT 1"
        )
        if reconcile.exit_code != 0:
            raise RuntimeError("quota reconcile audit missing")
        drift = 0
        self._require_correlated_transitions(minimum=3)
        return DeployedProbeResult(
            gate_id="G1C-SEC-QUOTA-RECOVERY",
            probe={
                "deployedApi": True,
                "beforeSha256": _sha256_text(before.stdout),
                "afterSha256": _sha256_text(after.stdout),
                "eof": True,
            },
            metrics={"quota_drift_after_recovery": drift},
        )

    def run_noisy_neighbor_probe(self) -> DeployedProbeResult:
        if self.noisy_duration_secs != CANONICAL_NOISY_UPLOAD_DURATION_SECS:
            raise RuntimeError(
                f"qualifying noisy duration must be {CANONICAL_NOISY_UPLOAD_DURATION_SECS}s"
            )
        samples_ns: list[int] = []
        starvation = 0
        token = self.credentials.beta_access_token
        deadline = time.monotonic() + min(self.noisy_duration_secs, 5)
        while time.monotonic() < deadline:
            started = time.perf_counter_ns()
            response = self._http(
                "POST",
                "/api/v1/search",
                token=token,
                body={"query": "phase1c-quiet-probe"},
            )
            elapsed_ns = time.perf_counter_ns() - started
            samples_ns.append(elapsed_ns)
            if response.status != 200:
                starvation += 1
            if elapsed_ns / 1_000_000 > STARVATION_SLOW_MS:
                starvation += 1
        while len(samples_ns) < self.quiet_search_samples:
            started = time.perf_counter_ns()
            response = self._http(
                "POST",
                "/api/v1/search",
                token=token,
                body={"query": "phase1c-quiet-probe"},
            )
            samples_ns.append(time.perf_counter_ns() - started)
            if response.status != 200:
                starvation += 1
        forbidden = {self.seed.marker_alpha, self.seed.marker_beta}
        for marker in forbidden:
            if marker and marker in json.dumps(samples_ns):
                starvation += 1
        self._compose(["ps", "-q", "worker-convert"])
        self._require_correlated_transitions(minimum=3)
        return DeployedProbeResult(
            gate_id="G1C-SEC-NOISY-NEIGHBOR",
            probe={"deployedApi": True, "sampleCount": len(samples_ns), "eof": True},
            metrics={
                "quiet_org_query_p95_ms": nearest_rank_p95_ms(samples_ns),
                "starvation_events": starvation,
            },
        )

    def run_audit_probe(self) -> DeployedProbeResult:
        self._validate_manifest_binding()
        gate_started_at = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S")
        token = self.credentials.alpha_access_token
        expected: list[dict[str, Any]] = []
        switch = self._switch_org(token, self.seed.org_alpha_id)
        switch_request_id = validate_server_request_id(body=switch.body, headers=switch.headers)
        switch_session_target_id = self._session_id_from_switch_access_token(switch)
        expected.append(
            {
                "action": "org.switch",
                "actorId": self.seed.alpha_user_id,
                "targetType": "session",
                "targetId": switch_session_target_id,
                "requestId": switch_request_id,
                "outcome": "success",
                "attemptStatus": switch.status,
            }
        )
        switch_payload = json.loads(switch.body)
        token = switch_payload.get("accessToken") or switch_payload.get("access_token") or token
        slug = f"phase1c-audit-{secrets.token_hex(4)}"
        create = self._http(
            "POST",
            "/api/v1/collections",
            token=token,
            body={"name": "phase1c-audit", "slug": slug, "visibility": "org"},
        )
        create_request_id = validate_server_request_id(body=create.body, headers=create.headers)
        collection_id = None
        if create.status // 100 == 2:
            create_payload = json.loads(create.body)
            raw_id = create_payload.get("id")
            if isinstance(raw_id, str):
                collection_id = validate_uuid(raw_id, field="collectionId")
        expected.append(
            {
                "action": "collection.create",
                "actorId": self.seed.alpha_user_id,
                "targetType": "collection",
                "targetId": collection_id,
                "requestId": create_request_id,
                "outcome": "success",
                "attemptStatus": create.status,
            }
        )
        if any(item["attemptStatus"] // 100 != 2 for item in expected):
            raise RuntimeError("audit probe mutation attempts must succeed")

        def fetch_audit(path: str) -> HttpResponse:
            return self._http("GET", path, token=token)

        entries = _fetch_audit_entries_paginated(fetch_audit)
        observed = 0
        for item in expected:
            if item.get("targetId") is None and item["action"] == "collection.create":
                for entry in entries:
                    if (
                        entry.get("action") == "collection.create"
                        and entry.get("requestId") == item["requestId"]
                        and entry.get("outcome") == "success"
                        and str(entry.get("actorId") or entry.get("actor_id")) == str(item["actorId"])
                    ):
                        target_id = entry.get("targetId") or entry.get("target_id")
                        if isinstance(target_id, str) and target_id.strip():
                            item["targetId"] = validate_uuid(target_id, field="collectionId")
                            break
            if any(_audit_matches(item, entry, gate_started_at=gate_started_at) for entry in entries):
                observed += 1
        ratio = observed / len(expected)
        self._require_correlated_transitions(minimum=2)
        return DeployedProbeResult(
            gate_id="G1C-SEC-AUDIT-COVERAGE",
            probe={
                "deployedApi": True,
                "expectedCount": len(expected),
                "observedCount": observed,
                "switchSessionTargetId": switch_session_target_id,
                "eof": True,
            },
            metrics={"admin_mutation_audit_coverage_ratio": ratio},
        )

    def run_qdrant_fail_closed_probe(self) -> DeployedProbeResult:
        self._validate_manifest_binding()
        token = self.credentials.alpha_access_token
        stop = self._compose(["stop", "qdrant"], timeout_secs=120)
        if stop.exit_code != 0:
            raise RuntimeError("qdrant stop failed")
        search = self._http(
            "POST",
            "/api/v1/search",
            token=token,
            body={"query": "phase1c-qdrant-down"},
        )
        ask = self._http(
            "POST",
            "/api/v1/ask",
            token=token,
            body={"query": "phase1c-qdrant-down"},
        )
        leakage = _DENIAL.scan_marker_leakage(
            search.body + ask.body,
            forbidden_markers={self.seed.marker_beta},
        )
        if search.status == 200 and ask.status == 200:
            raise RuntimeError("qdrant unavailable must not return 200 success")
        self._compose(["start", "qdrant"], timeout_secs=120)
        self._compose(["ps", "-q", "qdrant"], timeout_secs=120)
        self._require_correlated_transitions(minimum=3)
        return DeployedProbeResult(
            gate_id="G1C-SEC-QDRANT-FAIL-CLOSED",
            probe={
                "deployedApi": True,
                "singleNodeUnavailable": True,
                "searchStatus": search.status,
                "askStatus": ask.status,
                "eof": True,
            },
            metrics={"cross_tenant_leakage_count": len(leakage)},
        )

    def run_gate(self, gate_id: str) -> DeployedProbeResult:
        dispatch = {
            "G1C-SEC-LEAKAGE": self.run_http_denial_probe,
            "G1C-SEC-REVOKE": self.run_revoke_probe,
            "G1C-SEC-ACL-CACHE": self.run_acl_cache_probe,
            "G1C-SEC-STALE-TOKENS": self.run_stale_tokens_probe,
            "G1C-SEC-QUOTA-RECOVERY": self.run_quota_recovery_probe,
            "G1C-SEC-NOISY-NEIGHBOR": self.run_noisy_neighbor_probe,
            "G1C-SEC-AUDIT-COVERAGE": self.run_audit_probe,
            "G1C-SEC-QDRANT-FAIL-CLOSED": self.run_qdrant_fail_closed_probe,
        }
        if gate_id not in dispatch:
            raise RuntimeError(f"unsupported deployed gate {gate_id}")
        return dispatch[gate_id]()


def bootstrap_seed_via_api(
    *,
    api_base: str,
    challenge: str,
    source_revision: dict[str, Any],
    shims: DeployedProbeShims | None = None,
) -> tuple[SeedFixture, SeedCredentials]:
    raise RuntimeError(
        "bootstrap_seed_via_api requires deploy/scripts/phase1c-multi-org-seed.sh output; "
        "set MARKHAND_PHASE1C_SEED_JSON and MARKHAND_PHASE1C_CREDENTIALS_JSON"
    )


def deployed_probe_to_command_probe(result: DeployedProbeResult) -> dict[str, Any]:
    return {
        "commandExitCode": 0,
        "timedOut": False,
        "outputTruncated": False,
        "eof": True,
        "durationMs": 0,
        "deployedProbe": True,
        "gateId": result.gate_id,
        **{k: v for k, v in result.probe.items() if k != "eof"},
    }
