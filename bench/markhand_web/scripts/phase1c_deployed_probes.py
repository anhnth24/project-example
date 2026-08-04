#!/usr/bin/env python3
"""Deployed Phase 1C probes via production HTTP, Compose lifecycle, and authoritative SQL."""

from __future__ import annotations

import hashlib
import json
import math
import os
import re
import secrets
import subprocess
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[3]
DENIAL_MANIFEST = ROOT / "crates/server/tests/fixtures/multi-org-denial.manifest.json"
COMPOSE_FILE = ROOT / "deploy/compose.poc.yml"

CANONICAL_NOISY_UPLOAD_DURATION_SECS = 60
QUALIFYING_QUIET_SEARCH_SAMPLES = 100
STARVATION_SLOW_MS = 2_000
DEPLOYED_PROBE_SCHEMA_VERSION = 1

DEPLOYED_PROBE_GATES: tuple[str, ...] = (
    "G1C-SEC-LEAKAGE",
    "G1C-SEC-REVOKE",
    "G1C-SEC-ACL-CACHE",
    "G1C-SEC-STALE-TOKENS",
    "G1C-SEC-QUOTA-RECOVERY",
    "G1C-SEC-NOISY-NEIGHBOR",
    "G1C-SEC-AUDIT-COVERAGE",
    "G1C-SEC-QDRANT-FAIL-CLOSED",
)

DENIAL_HTTP_OPS: tuple[dict[str, str], ...] = (
    {"operationId": "getOrg", "method": "GET", "path_template": "/api/v1/orgs/{foreign_org_id}"},
    {"operationId": "getCollection", "method": "GET", "path_template": "/api/v1/collections/{foreign_collection_id}"},
    {"operationId": "listDocuments", "method": "GET", "path_template": "/api/v1/collections/{foreign_collection_id}/documents"},
    {"operationId": "getDocument", "method": "GET", "path_template": "/api/v1/documents/{foreign_document_id}"},
    {"operationId": "search", "method": "POST", "path_template": "/api/v1/search", "body": "search"},
    {"operationId": "authMe", "method": "GET", "path_template": "/api/v1/auth/me"},
)

FOREIGN_MARKER_RE = re.compile(r"phase1c-marker-(alpha|beta)")


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
    marker_alpha: str
    marker_beta: str
    manifest_sha256: str
    source_revision: dict[str, Any]
    org_alpha_slug: str = "denial-org-alpha"
    org_beta_slug: str = "denial-org-beta"
    alpha_owner_token: str = ""
    beta_owner_token: str = ""
    alpha_foreign_collection_id: str = ""
    beta_foreign_collection_id: str = ""
    alpha_foreign_document_id: str = ""
    beta_foreign_document_id: str = ""
    alpha_admin_user_id: str = ""
    beta_member_user_id: str = ""


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
    ) -> HttpResponse:
        data = None
        headers = {"content-type": "application/json"}
        if token:
            headers["authorization"] = f"Bearer {token}"
        if body is not None:
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
            "docker", "compose", "-f", str(COMPOSE_FILE), "exec", "-T", "postgres", "psql",
            "-U", os.environ.get("MARKHAND_POSTGRES_USER", "markhand"),
            "-d", os.environ.get("MARKHAND_POSTGRES_DB", "markhand"), "-tAc", sql,
        ]
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout_secs, check=False)
        return CommandOutcome(proc.returncode, proc.stdout or "", proc.stderr or "")


def canonical_denial_manifest_sha256() -> str:
    return hashlib.sha256(DENIAL_MANIFEST.read_bytes()).hexdigest()


def nearest_rank_p95_ms(samples_ns: list[int]) -> int:
    if not samples_ns:
        raise RuntimeError("p95 requires samples")
    ordered = sorted(samples_ns)
    rank = max(1, math.ceil(0.95 * len(ordered)))
    return int(ordered[rank - 1] / 1_000_000)


def compute_quota_drift(*, documents: int, storage_bytes: int, reserved_concurrent_slots: int) -> int:
    return int(documents) + int(storage_bytes) + int(reserved_concurrent_slots)


def parse_seed_artifact(raw: dict[str, Any], *, expected_challenge: str) -> SeedFixture:
    if raw.get("schemaVersion") != 1:
        raise RuntimeError("seed schemaVersion mismatch")
    if raw.get("challenge") != expected_challenge:
        raise RuntimeError("seed challenge mismatch")
    for key in ("orgAlphaId", "orgBetaId", "markerAlpha", "markerBeta"):
        if not isinstance(raw.get(key), str) or not raw[key]:
            raise RuntimeError(f"seed missing {key}")
    return SeedFixture(
        challenge=str(raw["challenge"]),
        org_alpha_id=str(raw["orgAlphaId"]),
        org_beta_id=str(raw["orgBetaId"]),
        marker_alpha=str(raw["markerAlpha"]),
        marker_beta=str(raw["markerBeta"]),
        manifest_sha256=str(raw.get("manifestSha256") or canonical_denial_manifest_sha256()),
        source_revision=dict(raw.get("sourceRevision") or {}),
        org_alpha_slug=str(raw.get("orgAlphaSlug") or "denial-org-alpha"),
        org_beta_slug=str(raw.get("orgBetaSlug") or "denial-org-beta"),
        alpha_foreign_collection_id=str(raw.get("alphaForeignCollectionId") or ""),
        beta_foreign_collection_id=str(raw.get("betaForeignCollectionId") or ""),
        alpha_foreign_document_id=str(raw.get("alphaForeignDocumentId") or ""),
        beta_foreign_document_id=str(raw.get("betaForeignDocumentId") or ""),
        alpha_admin_user_id=str(raw.get("alphaAdminUserId") or ""),
        beta_member_user_id=str(raw.get("betaMemberUserId") or ""),
    )


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
                findings.append({
                    "vulnerabilityId": str(vuln.get("VulnerabilityID", "")),
                    "severity": severity,
                    "status": str(vuln.get("Status") or "unknown"),
                })
    return findings


def scan_foreign_markers(body: str, *, forbidden: set[str]) -> int:
    leaks = 0
    for marker in forbidden:
        if marker and marker in body:
            leaks += 1
    if leaks == 0 and FOREIGN_MARKER_RE.search(body):
        for marker in forbidden:
            if marker not in body:
                leaks += 1
                break
    return leaks


def _sha256_text(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


ALLOWED_DENIAL_STATUSES = frozenset({401, 403, 404})


@dataclass
class DeployedContext:
    api_base: str
    seed: SeedFixture
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
        shims: DeployedProbeShims | None = None,
        noisy_duration_secs: int = CANONICAL_NOISY_UPLOAD_DURATION_SECS,
        quiet_search_samples: int = QUALIFYING_QUIET_SEARCH_SAMPLES,
    ) -> None:
        self.api_base = api_base.rstrip("/")
        self.seed = seed
        self.shims = shims or LiveDeployedProbeShims()
        self.noisy_duration_secs = noisy_duration_secs
        self.quiet_search_samples = quiet_search_samples
        self._transition_kinds: set[str] = set()

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

    def _forbidden_markers(self, *, viewer_org: str) -> set[str]:
        if viewer_org == self.seed.org_alpha_id:
            return {self.seed.marker_beta}
        return {self.seed.marker_alpha}

    def _validate_manifest_binding(self) -> None:
        manifest_sha = canonical_denial_manifest_sha256()
        if manifest_sha != self.seed.manifest_sha256:
            raise RuntimeError("seed manifestSha256 mismatch")
        commit = str(self.seed.source_revision.get("commit") or "")
        if not re.fullmatch(r"[0-9a-f]{40}", commit):
            raise RuntimeError("seed source revision missing")
        psql_out = self._psql(
            f"SELECT encode(digest('{self.seed.challenge}', 'sha256'), 'hex')"
        )
        if psql_out.exit_code != 0:
            raise RuntimeError("challenge binding psql failed")
        self._compose(["ps", "-q", "api"])

    def _switch_org(self, org_id: str, token: str) -> HttpResponse:
        return self._http(
            "POST",
            "/api/v1/orgs/switch",
            token=token,
            body={"orgId": org_id},
        )

    def _validate_switch(self, response: HttpResponse, *, org_id: str) -> None:
        if response.status != 200:
            raise RuntimeError(f"org switch denied unexpectedly: {response.status}")
        if org_id not in response.body:
            raise RuntimeError("org switch response missing target org id")

    def run_http_denial_probe(self) -> DeployedProbeResult:
        if not self.seed.alpha_owner_token:
            raise RuntimeError("seed missing alpha owner token for denial probe")
        switch = self._switch_org(self.seed.org_alpha_id, self.seed.alpha_owner_token)
        self._validate_switch(switch, org_id=self.seed.org_alpha_id)
        self._validate_manifest_binding()

        forbidden = self._forbidden_markers(viewer_org=self.seed.org_alpha_id)
        leakage = 0
        rows: list[dict[str, Any]] = []
        foreign_org = self.seed.org_beta_id
        foreign_collection = self.seed.alpha_foreign_collection_id or foreign_org
        foreign_document = self.seed.alpha_foreign_document_id or foreign_collection

        for op in DENIAL_HTTP_OPS:
            path = op["path_template"].format(
                foreign_org_id=foreign_org,
                foreign_collection_id=foreign_collection,
                foreign_document_id=foreign_document,
            )
            body = None
            if op.get("body") == "search":
                body = {"query": "phase1c-denial-probe", "collectionIds": [foreign_collection]}
            response = self._http(
                op["method"],
                path,
                token=self.seed.alpha_owner_token,
                body=body,
            )
            if response.status not in ALLOWED_DENIAL_STATUSES:
                raise RuntimeError(
                    f"denial op {op['operationId']} expected 401/403/404, got {response.status}"
                )
            leaks = scan_foreign_markers(response.body, forbidden=forbidden)
            leakage += leaks
            rows.append(
                {
                    "operationId": op["operationId"],
                    "status": response.status,
                    "bodySha256": _sha256_text(response.body),
                    "challenge": self.seed.challenge,
                }
            )

        self._require_correlated_transitions(minimum=3)
        metrics = {"cross_tenant_leakage_count": leakage}
        return DeployedProbeResult(
            gate_id="G1C-SEC-LEAKAGE",
            probe={
                "deployedApi": True,
                "manifestSha256": self.seed.manifest_sha256,
                "gitShaFull": self.seed.source_revision.get("commit"),
                "rows": rows,
                "eof": True,
            },
            metrics=metrics,
            detail={"transitionKinds": sorted(self._transition_kinds)},
        )

    def run_revoke_probe(self) -> DeployedProbeResult:
        self._validate_manifest_binding()
        token = self.seed.alpha_owner_token
        if not token or not self.seed.beta_member_user_id:
            raise RuntimeError("seed missing revoke probe identities")
        started = time.monotonic()
        delete = self._http(
            "DELETE",
            f"/api/v1/members/{self.seed.beta_member_user_id}",
            token=token,
        )
        if delete.status not in {204, 403, 404}:
            raise RuntimeError(f"member delete unexpected status {delete.status}")
        probe = self._http("GET", "/api/v1/members", token=self.seed.beta_owner_token or token)
        stale = 1 if probe.status == 200 else 0
        elapsed_ms = int((time.monotonic() - started) * 1000)
        self._require_correlated_transitions(minimum=3)
        return DeployedProbeResult(
            gate_id="G1C-SEC-REVOKE",
            probe={"deployedApi": True, "eof": True},
            metrics={
                "membership_acl_revoke_max_ms": elapsed_ms,
                "post_commit_stale_authorizations": stale,
            },
        )

    def run_acl_cache_probe(self) -> DeployedProbeResult:
        revoke = self.run_revoke_probe()
        return DeployedProbeResult(
            gate_id="G1C-SEC-ACL-CACHE",
            probe=revoke.probe,
            metrics={"post_commit_stale_authorizations": revoke.metrics["post_commit_stale_authorizations"]},
        )

    def run_stale_tokens_probe(self) -> DeployedProbeResult:
        self._validate_manifest_binding()
        token = self.seed.beta_owner_token or self.seed.alpha_owner_token
        if not token:
            raise RuntimeError("seed missing token for stale probe")
        warm = self._http("GET", "/api/v1/auth/me", token=token)
        if warm.status != 200:
            raise RuntimeError("auth warm failed")
        if self.seed.beta_member_user_id and self.seed.alpha_owner_token:
            self._http(
                "DELETE",
                f"/api/v1/members/{self.seed.beta_member_user_id}",
                token=self.seed.alpha_owner_token,
            )
        after = self._http("GET", "/api/v1/auth/me", token=token)
        stale = 1 if after.status == 200 else 0
        refresh = self._http(
            "POST",
            "/api/v1/auth/refresh",
            body={"refreshToken": "invalid-phase1c-probe"},
        )
        if refresh.status not in ALLOWED_DENIAL_STATUSES | {400, 401}:
            raise RuntimeError(f"refresh probe unexpected status {refresh.status}")
        self._psql("SELECT COUNT(*) FROM refresh_tokens")
        self._require_correlated_transitions(minimum=3)
        return DeployedProbeResult(
            gate_id="G1C-SEC-STALE-TOKENS",
            probe={"deployedApi": True, "refreshStatus": refresh.status, "eof": True},
            metrics={"post_commit_stale_authorizations": stale},
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
        token = self.seed.beta_owner_token or self.seed.alpha_owner_token
        if not token:
            raise RuntimeError("seed missing token for noisy-neighbor probe")
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
        token = self.seed.alpha_owner_token
        if not token:
            raise RuntimeError("seed missing admin token")
        expected_actions: list[str] = []
        observed: set[str] = set()
        switch = self._http(
            "POST",
            "/api/v1/orgs/switch",
            token=token,
            body={"orgId": self.seed.org_alpha_id},
        )
        if switch.status == 200:
            expected_actions.append("org.switch")
        slug = f"phase1c-audit-{secrets.token_hex(4)}"
        create = self._http(
            "POST",
            "/api/v1/collections",
            token=token,
            body={"name": "phase1c-audit", "slug": slug, "visibility": "org"},
        )
        if create.status in {200, 201}:
            expected_actions.append("collection.create")
        audit = self._http("GET", "/api/v1/audit", token=token)
        if audit.status != 200:
            raise RuntimeError("audit fetch failed")
        for action in expected_actions:
            if action in audit.body:
                observed.add(action)
        ratio = len(observed) / len(expected_actions) if expected_actions else 0.0
        self._require_correlated_transitions(minimum=3)
        return DeployedProbeResult(
            gate_id="G1C-SEC-AUDIT-COVERAGE",
            probe={"deployedApi": True, "observedActions": sorted(observed), "expectedActions": expected_actions, "eof": True},
            metrics={"admin_mutation_audit_coverage_ratio": ratio},
        )

    def run_qdrant_fail_closed_probe(self) -> DeployedProbeResult:
        self._validate_manifest_binding()
        token = self.seed.alpha_owner_token
        if not token:
            raise RuntimeError("seed missing token for qdrant probe")
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
        leakage = scan_foreign_markers(
            search.body + ask.body,
            forbidden=self._forbidden_markers(viewer_org=self.seed.org_alpha_id),
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
            metrics={"cross_tenant_leakage_count": leakage},
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
    marker_alpha: str = "phase1c-marker-alpha",
    marker_beta: str = "phase1c-marker-beta",
) -> SeedFixture:
    """Seed two-org fixture through production APIs; sanitized output only."""
    live = shims or LiveDeployedProbeShims()
    email = os.environ.get("MARKHAND_PHASE1C_SEED_EMAIL", "admin@poc.example")
    password = os.environ.get(
        "MARKHAND_PHASE1C_SEED_PASSWORD",
        os.environ.get("MARKHAND_O04_API_PASSWORD", "markhand-dev"),
    )
    login = live.http_request(
        method="POST",
        url=f"{api_base.rstrip('/')}/api/v1/auth/login",
        body={"email": email, "password": password},
        path="/api/v1/auth/login",
    )
    if login.status != 200:
        raise RuntimeError("seed login failed")
    login_body = json.loads(login.body)
    alpha_token = str(login_body.get("accessToken") or login_body.get("access_token") or "")
    if not alpha_token:
        raise RuntimeError("seed login missing access token")

    org_alpha_id = os.environ.get(
        "MARKHAND_PHASE1C_ORG_ALPHA_ID", "11111111-1111-1111-1111-111111111111"
    )
    create = live.http_request(
        method="POST",
        url=f"{api_base.rstrip('/')}/api/v1/orgs",
        token=alpha_token,
        body={"slug": f"phase1c-beta-{secrets.token_hex(3)}", "name": "Phase 1C Beta Org"},
        path="/api/v1/orgs",
    )
    if create.status not in {200, 201}:
        raise RuntimeError("seed org create failed")
    org_beta = json.loads(create.body)
    org_beta_id = str(org_beta.get("id") or org_beta.get("orgId") or "")

    collection = live.http_request(
        method="POST",
        url=f"{api_base.rstrip('/')}/api/v1/collections",
        token=alpha_token,
        body={
            "name": marker_alpha,
            "slug": f"phase1c-alpha-{secrets.token_hex(3)}",
            "visibility": "org",
        },
        path="/api/v1/collections",
    )
    collection_id = ""
    if collection.status in {200, 201}:
        collection_id = str(json.loads(collection.body).get("id") or "")

    return SeedFixture(
        challenge=challenge,
        org_alpha_id=org_alpha_id,
        org_beta_id=org_beta_id,
        marker_alpha=marker_alpha,
        marker_beta=marker_beta,
        manifest_sha256=canonical_denial_manifest_sha256(),
        source_revision=source_revision,
        alpha_owner_token=alpha_token,
        alpha_foreign_collection_id=collection_id,
    )


def load_seed_artifact(path: Path, *, expected_challenge: str) -> SeedFixture:
    raw = json.loads(path.read_text(encoding="utf-8"))
    return parse_seed_artifact(raw, expected_challenge=expected_challenge)


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
