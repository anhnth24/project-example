#!/usr/bin/env python3
"""Production API + controlled POC DB bootstrap for Phase 1C multi-org seed."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import secrets
import subprocess
import sys
import tempfile
import urllib.error
import urllib.request
import uuid
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "bench/markhand_web/scripts"))

from phase1c_deployed_probes import (  # noqa: E402
    build_public_seed_evidence,
    canonical_denial_manifest_sha256,
    purge_phase1c_credentials,
    validate_uuid,
)

COMPOSE_FILE = ROOT / "deploy/compose.poc.yml"
MULTIPART_BOUNDARY = "----markhandPhase1cSeedBoundary"
IDENTITY_FIXTURE_BOUNDARY = "phase1c-identity-fixture-boundary"
RESOURCE_FIXTURE_BOUNDARY = "phase1c-resource-fixture-boundary"
BETA_USER_ID = "33333333-3333-3333-3333-333333333301"
BETA_EMAIL = "phase1c-beta@poc.example"
DISPOSABLE_USER_ID = "44444444-4444-4444-4444-444444444401"
DISPOSABLE_EMAIL = "phase1c-disposable@poc.example"
ACCEPT_USER_ID = "55555555-5555-5555-5555-555555555501"
ACCEPT_EMAIL = "phase1c-accept@poc.example"
ALPHA_ORG_ID = "11111111-1111-1111-1111-111111111111"
ALPHA_USER_ID = "22222222-2222-2222-2222-222222222201"


def _sha256_text(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


def _git_sha() -> str:
    env_sha = os.environ.get("GITHUB_SHA", "").strip()
    if env_sha:
        return env_sha
    return subprocess.check_output(["git", "-C", str(ROOT), "rev-parse", "HEAD"], text=True).strip()


def _http(
    *,
    api_base: str,
    method: str,
    path: str,
    token: str | None = None,
    body: dict[str, Any] | None = None,
    content_type: str = "application/json",
    raw_body: bytes | None = None,
    extra_headers: dict[str, str] | None = None,
) -> tuple[int, dict[str, str], str]:
    headers = {"content-type": content_type}
    if token:
        headers["authorization"] = f"Bearer {token}"
    if extra_headers:
        headers.update(extra_headers)
    data = raw_body
    if data is None and body is not None:
        data = json.dumps(body).encode("utf-8")
    request = urllib.request.Request(f"{api_base.rstrip('/')}{path}", data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(request, timeout=120) as response:
            raw = response.read().decode("utf-8", errors="replace")
            return response.status, dict(response.headers), raw
    except urllib.error.HTTPError as error:
        raw = error.read().decode("utf-8", errors="replace")
        return error.code, dict(error.headers), raw


def _psql(sql: str) -> str:
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
        "-v",
        "ON_ERROR_STOP=1",
        "-tAc",
        sql,
    ]
    proc = subprocess.run(cmd, capture_output=True, text=True, check=False)
    if proc.returncode != 0:
        raise RuntimeError("poc db bootstrap failed")
    return (proc.stdout or "").strip()


def _bootstrap_identity_users(password_hash_sql: str) -> None:
    # IDENTITY_FIXTURE_BOUNDARY: no registration API; bootstrap login-capable users only.
    _psql(
        f"""
        BEGIN;
        SET LOCAL app.org_id = '{ALPHA_ORG_ID}';
        INSERT INTO users (id, email, display_name, password_hash)
        VALUES ('{BETA_USER_ID}', '{BETA_EMAIL}', 'Phase 1C Beta', '{password_hash_sql}')
        ON CONFLICT (id) DO UPDATE SET email = EXCLUDED.email, password_hash = EXCLUDED.password_hash;
        INSERT INTO org_memberships (org_id, user_id, role, state)
        VALUES ('{ALPHA_ORG_ID}', '{BETA_USER_ID}', 'viewer', 'active')
        ON CONFLICT (org_id, user_id) DO UPDATE SET role = EXCLUDED.role, state = 'active';
        INSERT INTO users (id, email, display_name, password_hash)
        VALUES ('{DISPOSABLE_USER_ID}', '{DISPOSABLE_EMAIL}', 'Phase 1C Disposable', '{password_hash_sql}')
        ON CONFLICT (id) DO UPDATE SET email = EXCLUDED.email, password_hash = EXCLUDED.password_hash;
        INSERT INTO users (id, email, display_name, password_hash)
        VALUES ('{ACCEPT_USER_ID}', '{ACCEPT_EMAIL}', 'Phase 1C Accept', '{password_hash_sql}')
        ON CONFLICT (id) DO UPDATE SET email = EXCLUDED.email, password_hash = EXCLUDED.password_hash;
        COMMIT;
        """
    )
    user_ok = _psql(f"SELECT COUNT(*) FROM users WHERE id = '{BETA_USER_ID}' AND email = '{BETA_EMAIL}'")
    membership_ok = _psql(
        f"SELECT COUNT(*) FROM org_memberships WHERE org_id = '{ALPHA_ORG_ID}' AND user_id = '{BETA_USER_ID}' AND state = 'active'"
    )
    disposable_ok = _psql(f"SELECT COUNT(*) FROM users WHERE id = '{DISPOSABLE_USER_ID}' AND email = '{DISPOSABLE_EMAIL}'")
    accept_ok = _psql(f"SELECT COUNT(*) FROM users WHERE id = '{ACCEPT_USER_ID}' AND email = '{ACCEPT_EMAIL}'")
    if user_ok != "1" or membership_ok != "1" or disposable_ok != "1" or accept_ok != "1":
        raise RuntimeError("identity fixture bootstrap validation failed")


def _claim_pair_for_org(org_id: str) -> tuple[str, str]:
    claim_low = str(uuid.uuid5(uuid.NAMESPACE_OID, f"{org_id}:claim-a"))
    claim_high = str(uuid.uuid5(uuid.NAMESPACE_OID, f"{org_id}:claim-b"))
    if claim_low > claim_high:
        claim_low, claim_high = claim_high, claim_low
    if claim_low == claim_high:
        raise RuntimeError("claim pair generation produced identical ids")
    return claim_low, claim_high


def _bootstrap_conflict_fixture(
    *,
    org_id: str,
    document_id: str,
    version_id: str,
    conflict_id: str,
) -> tuple[str, str]:
    # RESOURCE_FIXTURE_BOUNDARY: conflicts have no production create HTTP route.
    # Migration 0006 claims columns + 0007 ck_conflicts__canonical_pair (claim_a_id < claim_b_id).
    claim_low, claim_high = _claim_pair_for_org(org_id)
    claim_pair_order = "claim_a_id < claim_b_id"
    if not (claim_low < claim_high):
        raise RuntimeError(f"conflict fixture violates {claim_pair_order}")
    effective_from = "2026-08-04T00:00:00Z"
    _psql(
        f"""
        BEGIN;
        SET LOCAL app.org_id = '{org_id}';
        INSERT INTO claims (
            id, org_id, document_id, version_id, claim_key, subject, predicate,
            value_type, value_money, effective_from
        ) VALUES
            (
                '{claim_low}', '{org_id}', '{document_id}', '{version_id}',
                'amount', 'contract', 'total', 'money', 100, '{effective_from}'
            ),
            (
                '{claim_high}', '{org_id}', '{document_id}', '{version_id}',
                'amount', 'contract', 'total', 'money', 200, '{effective_from}'
            )
        ON CONFLICT (id) DO NOTHING;
        INSERT INTO conflicts (
            id, org_id, status, severity, conflict_type, claim_a_id, claim_b_id, first_detected_version_id
        ) VALUES (
            '{conflict_id}', '{org_id}', 'open', 'warning', 'numeric', '{claim_low}', '{claim_high}', '{version_id}'
        ) ON CONFLICT (id) DO NOTHING;
        INSERT INTO conflict_evidence (org_id, conflict_id, claim_id, evidence_role, citation_quote)
        VALUES
            ('{org_id}', '{conflict_id}', '{claim_low}', 'left', '100'),
            ('{org_id}', '{conflict_id}', '{claim_high}', 'right', '200')
        ON CONFLICT DO NOTHING;
        COMMIT;
        """
    )
    ok = _psql(f"SELECT COUNT(*) FROM conflicts WHERE id = '{conflict_id}' AND org_id = '{org_id}'")
    if ok != "1":
        raise RuntimeError("conflict fixture bootstrap validation failed")
    return claim_low, claim_high


def _login(api_base: str, email: str, password: str) -> dict[str, Any]:
    status, _, body = _http(
        api_base=api_base,
        method="POST",
        path="/api/v1/auth/login",
        body={"email": email, "password": password},
    )
    if status != 200:
        raise RuntimeError("seed login failed")
    payload = json.loads(body)
    if not isinstance(payload, dict):
        raise RuntimeError("login response invalid")
    return payload


def _auth_me(api_base: str, access_token: str) -> dict[str, Any]:
    status, _, body = _http(
        api_base=api_base,
        method="GET",
        path="/api/v1/auth/me",
        token=access_token,
    )
    if status != 200:
        raise RuntimeError("auth/me failed during seed")
    payload = json.loads(body)
    if not isinstance(payload, dict):
        raise RuntimeError("auth/me response invalid")
    return payload


def _auth_me_session_id(api_base: str, access_token: str) -> str:
    payload = _auth_me(api_base, access_token)
    session_id = payload.get("sessionId") or payload.get("session_id")
    if not isinstance(session_id, str) or not session_id.strip():
        raise RuntimeError("auth/me missing sessionId")
    return validate_uuid(session_id, field="sessionId")


def _switch_org(api_base: str, token: str, org_id: str) -> tuple[str, str, str]:
    status, _, body = _http(
        api_base=api_base,
        method="POST",
        path="/api/v1/orgs/switch",
        token=token,
        body={"orgId": org_id},
    )
    if status // 100 != 2:
        raise RuntimeError("org switch failed during seed")
    payload = json.loads(body)
    access = payload.get("accessToken") or payload.get("access_token")
    refresh = payload.get("refreshToken") or payload.get("refresh_token")
    if not isinstance(access, str) or not isinstance(refresh, str):
        raise RuntimeError("org switch missing token pair")
    _auth_me(api_base, access)
    return access, refresh, validate_uuid(_auth_me_session_id(api_base, access), field="sessionId")


def _multipart_upload(
    *,
    api_base: str,
    token: str,
    collection_id: str,
    filename: str,
    content: bytes,
) -> dict[str, Any]:
    body = bytearray()
    body.extend(
        f'--{MULTIPART_BOUNDARY}\r\nContent-Disposition: form-data; name="collectionId"\r\n\r\n{collection_id}\r\n'.encode(
            "utf-8"
        )
    )
    body.extend(
        (
            f'--{MULTIPART_BOUNDARY}\r\nContent-Disposition: form-data; name="file"; filename="{filename}"\r\n'
            f"Content-Type: text/plain\r\n\r\n"
        ).encode("utf-8")
    )
    body.extend(content)
    body.extend(f"\r\n--{MULTIPART_BOUNDARY}--\r\n".encode("utf-8"))
    status, _, raw = _http(
        api_base=api_base,
        method="POST",
        path="/api/v1/uploads",
        token=token,
        content_type=f"multipart/form-data; boundary={MULTIPART_BOUNDARY}",
        raw_body=bytes(body),
        extra_headers={"idempotency-key": f"phase1c-seed-{secrets.token_hex(4)}"},
    )
    if status not in {200, 201}:
        raise RuntimeError("multipart upload failed")
    payload = json.loads(raw)
    for key in ("documentId", "versionId"):
        if not isinstance(payload.get(key), str):
            raise RuntimeError(f"upload response missing {key}")
    return payload


def _publish_version(*, api_base: str, token: str, document_id: str, version_id: str) -> None:
    status, _, _ = _http(
        api_base=api_base,
        method="POST",
        path=f"/api/v1/documents/{document_id}/versions/{version_id}/publish",
        token=token,
        body={},
    )
    if status not in {200, 204}:
        raise RuntimeError("publish version failed")


def _issue_download_capability(*, api_base: str, token: str, document_id: str, version_id: str) -> str:
    status, _, raw = _http(
        api_base=api_base,
        method="POST",
        path=f"/api/v1/documents/{document_id}/versions/{version_id}/download-capability",
        token=token,
        body={"purpose": "markdown"},
    )
    if status != 200:
        raise RuntimeError("issue download capability failed")
    payload = json.loads(raw)
    capability = payload.get("capability")
    if not isinstance(capability, str) or not capability.strip():
        raise RuntimeError("download capability response missing capability")
    return capability


def _create_project(*, api_base: str, token: str, name: str) -> str:
    status, _, raw = _http(
        api_base=api_base,
        method="POST",
        path="/api/v1/projects",
        token=token,
        body={"name": name},
    )
    if status not in {200, 201}:
        raise RuntimeError("project create failed")
    project_id = json.loads(raw).get("id")
    if not isinstance(project_id, str):
        raise RuntimeError("project response missing id")
    return validate_uuid(project_id, field="projectId")


def _create_chat_session(*, api_base: str, token: str, title: str) -> str:
    status, _, raw = _http(
        api_base=api_base,
        method="POST",
        path="/api/v1/chat-sessions",
        token=token,
        body={"title": title},
    )
    if status not in {200, 201}:
        raise RuntimeError("chat session create failed")
    session_id = json.loads(raw).get("id")
    if not isinstance(session_id, str):
        raise RuntimeError("chat session response missing id")
    return validate_uuid(session_id, field="chatSessionId")


def _create_collection(*, api_base: str, token: str, marker: str, slug_prefix: str) -> str:
    status, _, raw = _http(
        api_base=api_base,
        method="POST",
        path="/api/v1/collections",
        token=token,
        body={
            "name": marker,
            "slug": f"{slug_prefix}-{secrets.token_hex(3)}",
            "visibility": "org",
        },
    )
    if status not in {200, 201}:
        raise RuntimeError("collection create failed")
    collection_id = json.loads(raw).get("id")
    if not isinstance(collection_id, str):
        raise RuntimeError("collection response missing id")
    return validate_uuid(collection_id, field="collectionId")


def _verify_conflict(api_base: str, token: str, conflict_id: str) -> None:
    list_status, _, list_raw = _http(api_base=api_base, method="GET", path="/api/v1/conflicts", token=token)
    if list_status != 200:
        raise RuntimeError("conflict list verification failed")
    list_payload = json.loads(list_raw)
    items = list_payload.get("items") or []
    if not any(isinstance(item, dict) and item.get("id") == conflict_id for item in items):
        raise RuntimeError("conflict missing from list endpoint")
    get_status, _, _ = _http(
        api_base=api_base,
        method="GET",
        path=f"/api/v1/conflicts/{conflict_id}",
        token=token,
    )
    if get_status != 200:
        raise RuntimeError("conflict get verification failed")


def _load_citation_fixture(
    *,
    api_base: str,
    token: str,
    org_id: str,
    document_id: str,
    version_id: str,
    marker: str,
) -> dict[str, Any]:
    preview_status, _, _ = _http(
        api_base=api_base,
        method="GET",
        path=f"/api/v1/documents/{document_id}/preview?versionId={version_id}",
        token=token,
    )
    if preview_status != 200:
        raise RuntimeError("citation fixture preview failed")
    chunk_row = _psql(
        f"""
        SELECT c.id::text || '|' || coalesce(c.span_start, 0)::text || '|' || coalesce(c.span_end, {len(marker)})::text || '|' || c.body
        FROM chunks c
        WHERE c.org_id = '{org_id}' AND c.document_id = '{document_id}' AND c.version_id = '{version_id}'
        ORDER BY c.ordinal ASC
        LIMIT 1
        """
    )
    if not chunk_row.strip():
        raise RuntimeError("citation fixture missing chunk row")
    chunk_id, span_start, span_end, body = chunk_row.split("|", 3)
    source_sha = _psql(
        f"SELECT source_content_sha256 FROM document_versions WHERE org_id = '{org_id}' AND document_id = '{document_id}' AND id = '{version_id}'"
    )
    markdown_sha = _psql(
        f"SELECT canonical_markdown_sha256 FROM document_versions WHERE org_id = '{org_id}' AND document_id = '{document_id}' AND id = '{version_id}'"
    )
    if not source_sha.strip() or not markdown_sha.strip():
        raise RuntimeError("citation fixture missing version hashes")
    quote = body.strip() or marker
    quote_len = len(quote)
    return {
        "betaCitationChunkId": validate_uuid(chunk_id, field="betaCitationChunkId"),
        "betaCitationSourceContentSha256": source_sha.strip(),
        "betaCitationCanonicalMarkdownSha256": markdown_sha.strip(),
        "betaCitationSourceSpanStart": int(span_start),
        "betaCitationSourceSpanEnd": int(span_end),
        "betaCitationQuoteLocalStart": 0,
        "betaCitationQuoteLocalEnd": quote_len,
        "betaCitationQuote": quote,
    }


def _atomic_write(path: Path, payload: dict[str, Any], *, mode: int = 0o644) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    serialized = json.dumps(payload, indent=2, sort_keys=True) + "\n"
    fd, tmp = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    tmp_path = Path(tmp)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            handle.write(serialized)
            handle.flush()
            os.fsync(handle.fileno())
        os.chmod(tmp_path, mode)
        tmp_path.replace(path)
    finally:
        if tmp_path.exists() and not path.exists():
            tmp_path.unlink(missing_ok=True)


def run_seed() -> int:
    api_base = os.environ.get("MARKHAND_API_BASE", "http://127.0.0.1:8788")
    alpha_email = os.environ.get("MARKHAND_PHASE1C_SEED_EMAIL", "admin@poc.example")
    password = os.environ.get(
        "MARKHAND_PHASE1C_SEED_PASSWORD",
        os.environ.get("MARKHAND_O04_API_PASSWORD", "markhand-dev"),
    )
    challenge = os.environ.get("MARKHAND_PHASE1C_CHALLENGE") or f"phase1c-seed-{secrets.token_hex(8)}"
    marker_alpha = os.environ.get("MARKHAND_PHASE1C_MARKER_ALPHA") or f"phase1c-marker-alpha-{secrets.token_hex(4)}"
    marker_beta = os.environ.get("MARKHAND_PHASE1C_MARKER_BETA") or f"phase1c-marker-beta-{secrets.token_hex(4)}"
    out = Path(os.environ.get("MARKHAND_PHASE1C_SEED_JSON", ROOT / ".artifacts/phase1c-multi-org-seed.json"))
    creds_out = Path(
        os.environ.get(
            "MARKHAND_PHASE1C_CREDENTIALS_JSON",
            out.with_name("phase1c-multi-org-seed.credentials.json"),
        )
    )
    git_sha = _git_sha()
    source_revision = {"commit": git_sha, "dirty": False}

    hash_proc = subprocess.run(
        ["cargo", "run", "-q", "-p", "fileconv-server", "--bin", "dev-hash-password", "--", password],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    if hash_proc.returncode != 0:
        raise RuntimeError("password hash generation failed")
    password_hash = hash_proc.stdout.strip().replace("'", "''")
    _bootstrap_identity_users(password_hash)

    alpha_login = _login(api_base, alpha_email, password)
    alpha_access = alpha_login.get("accessToken") or alpha_login.get("access_token")
    alpha_refresh = alpha_login.get("refreshToken") or alpha_login.get("refresh_token")
    if not isinstance(alpha_access, str) or not isinstance(alpha_refresh, str):
        raise RuntimeError("alpha login missing token pair")
    _auth_me(api_base, alpha_access)

    status, _, body = _http(
        api_base=api_base,
        method="POST",
        path="/api/v1/orgs",
        token=alpha_access,
        body={"slug": f"phase1c-beta-{secrets.token_hex(3)}", "name": "Phase 1C Beta Org"},
    )
    if status not in {200, 201}:
        raise RuntimeError("beta org create failed")
    beta_org = json.loads(body)
    org_beta_id = beta_org.get("id") or beta_org.get("orgId")
    org_beta_slug = beta_org.get("slug")
    if not isinstance(org_beta_id, str) or not isinstance(org_beta_slug, str):
        raise RuntimeError("beta org response missing id/slug")
    org_beta_id = validate_uuid(org_beta_id, field="orgBetaId")

    alpha_beta_access, alpha_beta_refresh, _ = _switch_org(api_base, alpha_access, org_beta_id)

    beta_org_invite_body = {"email": BETA_EMAIL, "role": "editor"}
    beta_org_invite_status, _, beta_org_invite_raw = _http(
        api_base=api_base,
        method="POST",
        path="/api/v1/members/invites",
        token=alpha_beta_access,
        body=beta_org_invite_body,
    )
    if beta_org_invite_status not in {200, 201}:
        raise RuntimeError("beta org member invite failed")
    beta_org_invite_payload = json.loads(beta_org_invite_raw)
    beta_org_invite = beta_org_invite_payload.get("invite") or {}
    beta_invite_id = beta_org_invite.get("id")
    beta_invite_token = beta_org_invite_payload.get("token")
    if not isinstance(beta_invite_id, str) or not isinstance(beta_invite_token, str):
        raise RuntimeError("beta org invite response missing id/token")

    beta_login = _login(api_base, BETA_EMAIL, password)
    beta_alpha_access = beta_login.get("accessToken") or beta_login.get("access_token")
    beta_alpha_refresh = beta_login.get("refreshToken") or beta_login.get("refresh_token")
    if not isinstance(beta_alpha_access, str) or not isinstance(beta_alpha_refresh, str):
        raise RuntimeError("beta login missing token pair")
    _auth_me(api_base, beta_alpha_access)

    accept_status, _, _ = _http(
        api_base=api_base,
        method="POST",
        path="/api/v1/members/invites/accept",
        token=beta_alpha_access,
        body={"token": beta_invite_token},
    )
    if accept_status not in {200, 201}:
        raise RuntimeError("beta org invite accept failed")

    beta_org_access, beta_org_refresh, beta_session_id = _switch_org(api_base, beta_alpha_access, org_beta_id)

    alpha_access, alpha_refresh, alpha_session_id = _switch_org(api_base, alpha_access, ALPHA_ORG_ID)

    alpha_collection_id = _create_collection(
        api_base=api_base,
        token=alpha_access,
        marker=marker_alpha,
        slug_prefix="phase1c-alpha",
    )
    beta_collection_id = _create_collection(
        api_base=api_base,
        token=beta_org_access,
        marker=marker_beta,
        slug_prefix="phase1c-beta",
    )
    beta_denial_disposable_collection_id = _create_collection(
        api_base=api_base,
        token=beta_org_access,
        marker=f"{marker_beta}-disposable-delete",
        slug_prefix="phase1c-beta-denial-del",
    )
    beta_denial_disposable_collection_update_id = _create_collection(
        api_base=api_base,
        token=beta_org_access,
        marker=f"{marker_beta}-disposable-update",
        slug_prefix="phase1c-beta-denial-upd",
    )

    accept_invite_status, _, accept_invite_raw = _http(
        api_base=api_base,
        method="POST",
        path="/api/v1/members/invites",
        token=beta_org_access,
        body={"email": ACCEPT_EMAIL, "role": "viewer"},
    )
    if accept_invite_status not in {200, 201}:
        raise RuntimeError("accept member invite failed")
    accept_invite_payload = json.loads(accept_invite_raw)
    beta_denial_accept_invite_token = accept_invite_payload.get("token")
    if not isinstance(beta_denial_accept_invite_token, str):
        raise RuntimeError("accept invite response missing token")
    accept_login = _login(api_base, ACCEPT_EMAIL, password)
    beta_denial_accept_access_token = accept_login.get("accessToken") or accept_login.get("access_token")
    if not isinstance(beta_denial_accept_access_token, str):
        raise RuntimeError("accept identity login missing access token")

    disposable_invite_status, _, disposable_invite_raw = _http(
        api_base=api_base,
        method="POST",
        path="/api/v1/members/invites",
        token=beta_org_access,
        body={"email": DISPOSABLE_EMAIL, "role": "viewer"},
    )
    if disposable_invite_status not in {200, 201}:
        raise RuntimeError("disposable member invite failed")
    disposable_invite_payload = json.loads(disposable_invite_raw)
    disposable_invite = disposable_invite_payload.get("invite") or {}
    beta_denial_disposable_invite_id = disposable_invite.get("id")
    disposable_invite_token = disposable_invite_payload.get("token")
    if not isinstance(beta_denial_disposable_invite_id, str) or not isinstance(disposable_invite_token, str):
        raise RuntimeError("disposable invite response missing id/token")

    disposable_login = _login(api_base, DISPOSABLE_EMAIL, password)
    disposable_access = disposable_login.get("accessToken") or disposable_login.get("access_token")
    if not isinstance(disposable_access, str):
        raise RuntimeError("disposable login missing access token")
    accept_disposable_status, _, _ = _http(
        api_base=api_base,
        method="POST",
        path="/api/v1/members/invites/accept",
        token=disposable_access,
        body={"token": disposable_invite_token},
    )
    if accept_disposable_status not in {200, 201}:
        raise RuntimeError("disposable invite accept failed")
    beta_denial_disposable_member_user_id = DISPOSABLE_USER_ID

    alpha_upload = _multipart_upload(
        api_base=api_base,
        token=alpha_access,
        collection_id=alpha_collection_id,
        filename="phase1c-alpha.txt",
        content=f"{marker_alpha}\n".encode("utf-8"),
    )
    beta_upload = _multipart_upload(
        api_base=api_base,
        token=beta_org_access,
        collection_id=beta_collection_id,
        filename="phase1c-beta.txt",
        content=f"{marker_beta}\n".encode("utf-8"),
    )
    disposable_upload = _multipart_upload(
        api_base=api_base,
        token=beta_org_access,
        collection_id=beta_denial_disposable_collection_id,
        filename="phase1c-beta-disposable.txt",
        content=b"phase1c-disposable\n",
    )
    alpha_document_id = validate_uuid(alpha_upload["documentId"], field="alphaDocumentId")
    beta_document_id = validate_uuid(beta_upload["documentId"], field="betaDocumentId")
    beta_denial_disposable_document_id = validate_uuid(
        disposable_upload["documentId"], field="betaDenialDisposableDocumentId"
    )
    alpha_version_id = validate_uuid(alpha_upload["versionId"], field="alphaVersionId")
    beta_version_id = validate_uuid(beta_upload["versionId"], field="betaVersionId")
    alpha_job_id = alpha_upload.get("jobId") or ""
    beta_job_id = beta_upload.get("jobId") or ""
    if not isinstance(alpha_job_id, str):
        alpha_job_id = ""
    if not isinstance(beta_job_id, str):
        beta_job_id = ""
    if alpha_job_id:
        validate_uuid(alpha_job_id, field="alphaJobId")
    if beta_job_id:
        validate_uuid(beta_job_id, field="betaJobId")

    _publish_version(
        api_base=api_base,
        token=alpha_access,
        document_id=alpha_document_id,
        version_id=alpha_version_id,
    )
    _publish_version(
        api_base=api_base,
        token=beta_org_access,
        document_id=beta_document_id,
        version_id=beta_version_id,
    )
    _publish_version(
        api_base=api_base,
        token=beta_org_access,
        document_id=beta_denial_disposable_document_id,
        version_id=validate_uuid(disposable_upload["versionId"], field="betaDenialDisposableVersionId"),
    )
    alpha_download_capability = _issue_download_capability(
        api_base=api_base,
        token=alpha_access,
        document_id=alpha_document_id,
        version_id=alpha_version_id,
    )
    beta_download_capability = _issue_download_capability(
        api_base=api_base,
        token=beta_org_access,
        document_id=beta_document_id,
        version_id=beta_version_id,
    )
    alpha_project_id = _create_project(
        api_base=api_base, token=alpha_access, name=f"phase1c-alpha-project-{secrets.token_hex(3)}"
    )
    beta_project_id = _create_project(
        api_base=api_base, token=beta_org_access, name=f"phase1c-beta-project-{secrets.token_hex(3)}"
    )
    alpha_chat_session_id = _create_chat_session(
        api_base=api_base, token=alpha_access, title=f"phase1c-alpha-chat-{secrets.token_hex(3)}"
    )
    beta_chat_session_id = _create_chat_session(
        api_base=api_base, token=beta_org_access, title=f"phase1c-beta-chat-{secrets.token_hex(3)}"
    )
    beta_denial_disposable_chat_session_id = _create_chat_session(
        api_base=api_base, token=beta_org_access, title=f"phase1c-beta-disposable-chat-{secrets.token_hex(3)}"
    )

    alpha_conflict_id = str(uuid.uuid4())
    beta_conflict_id = str(uuid.uuid4())
    beta_denial_disposable_conflict_id = str(uuid.uuid4())
    _bootstrap_conflict_fixture(
        org_id=ALPHA_ORG_ID,
        document_id=alpha_document_id,
        version_id=alpha_version_id,
        conflict_id=alpha_conflict_id,
    )
    _bootstrap_conflict_fixture(
        org_id=org_beta_id,
        document_id=beta_document_id,
        version_id=beta_version_id,
        conflict_id=beta_conflict_id,
    )
    _bootstrap_conflict_fixture(
        org_id=org_beta_id,
        document_id=beta_denial_disposable_document_id,
        version_id=validate_uuid(disposable_upload["versionId"], field="betaDenialDisposableVersionId"),
        conflict_id=beta_denial_disposable_conflict_id,
    )
    _verify_conflict(api_base, alpha_access, alpha_conflict_id)
    _verify_conflict(api_base, beta_org_access, beta_conflict_id)
    _verify_conflict(api_base, beta_org_access, beta_denial_disposable_conflict_id)

    citation_fixture = _load_citation_fixture(
        api_base=api_base,
        token=beta_org_access,
        org_id=org_beta_id,
        document_id=beta_document_id,
        version_id=beta_version_id,
        marker=marker_beta,
    )

    seed_raw: dict[str, Any] = {
        "schemaVersion": 1,
        "challenge": challenge,
        "sourceRevision": source_revision,
        "manifestSha256": canonical_denial_manifest_sha256(),
        "identityFixtureBoundary": IDENTITY_FIXTURE_BOUNDARY,
        "resourceFixtureBoundary": RESOURCE_FIXTURE_BOUNDARY,
        "orgAlphaId": ALPHA_ORG_ID,
        "orgBetaId": org_beta_id,
        "alphaUserId": ALPHA_USER_ID,
        "betaUserId": BETA_USER_ID,
        "markerAlpha": marker_alpha,
        "markerBeta": marker_beta,
        "alphaCollectionId": alpha_collection_id,
        "betaCollectionId": beta_collection_id,
        "alphaDocumentId": alpha_document_id,
        "betaDocumentId": beta_document_id,
        "alphaJobId": alpha_job_id,
        "betaJobId": beta_job_id,
        "alphaVersionId": alpha_version_id,
        "betaVersionId": beta_version_id,
        "alphaChatSessionId": alpha_chat_session_id,
        "betaChatSessionId": beta_chat_session_id,
        "alphaProjectId": alpha_project_id,
        "betaProjectId": beta_project_id,
        "alphaConflictId": alpha_conflict_id,
        "betaConflictId": beta_conflict_id,
        "betaMemberUserId": BETA_USER_ID,
        "alphaInviteId": beta_invite_id,
        "betaInviteId": beta_invite_id,
        "betaDownloadCapabilityHash": _sha256_text(beta_download_capability),
        "alphaDownloadCapabilityHash": _sha256_text(alpha_download_capability),
        "betaInviteTokenHash": _sha256_text(beta_invite_token),
        "alphaSessionIdHash": _sha256_text(alpha_session_id),
        "betaSessionIdHash": _sha256_text(beta_session_id),
        "orgAlphaSlug": "poc",
        "orgBetaSlug": org_beta_slug,
    }
    evidence = build_public_seed_evidence(seed_raw)
    credentials = {
        "schemaVersion": 1,
        "challenge": challenge,
        "alphaAccessToken": alpha_access,
        "alphaRefreshToken": alpha_refresh,
        "betaAccessToken": beta_org_access,
        "betaRefreshToken": beta_org_refresh,
        "betaAlphaAccessToken": beta_alpha_access,
        "betaAlphaRefreshToken": beta_alpha_refresh,
        "alphaBetaAccessToken": alpha_beta_access,
        "alphaBetaRefreshToken": alpha_beta_refresh,
        "alphaSessionId": alpha_session_id,
        "betaSessionId": beta_session_id,
        "betaInviteToken": beta_invite_token,
        "alphaDownloadCapability": alpha_download_capability,
        "betaDownloadCapability": beta_download_capability,
        "betaDenialDisposableCollectionId": beta_denial_disposable_collection_id,
        "betaDenialDisposableCollectionUpdateId": beta_denial_disposable_collection_update_id,
        "betaDenialDisposableDocumentId": beta_denial_disposable_document_id,
        "betaDenialDisposableChatSessionId": beta_denial_disposable_chat_session_id,
        "betaDenialDisposableInviteId": beta_denial_disposable_invite_id,
        "betaDenialDisposableMemberUserId": beta_denial_disposable_member_user_id,
        "betaDenialDisposableConflictId": beta_denial_disposable_conflict_id,
        "betaDenialAcceptInviteToken": beta_denial_accept_invite_token,
        "betaDenialAcceptAccessToken": beta_denial_accept_access_token,
        **citation_fixture,
    }
    _atomic_write(out, evidence, mode=0o644)
    _atomic_write(creds_out, credentials, mode=0o600)
    try:
        creds_out.chmod(0o600)
    except OSError:
        pass
    print("PHASE1C_SEED_COMPLETE")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        return 0
    creds_path = Path(
        os.environ.get(
            "MARKHAND_PHASE1C_CREDENTIALS_JSON",
            ROOT / ".artifacts/phase1c-multi-org-seed.credentials.json",
        )
    )

    def _cleanup_credentials() -> None:
        purge_phase1c_credentials(creds_path)

    import signal

    signal.signal(signal.SIGHUP, lambda *_: (_cleanup_credentials(), sys.exit(129)))
    signal.signal(signal.SIGINT, lambda *_: (_cleanup_credentials(), sys.exit(130)))
    signal.signal(signal.SIGTERM, lambda *_: (_cleanup_credentials(), sys.exit(143)))
    try:
        return run_seed()
    except Exception as error:
        print(f"seed failed: {error}", file=sys.stderr)
        _cleanup_credentials()
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
