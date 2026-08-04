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
)

COMPOSE_FILE = ROOT / "deploy/compose.poc.yml"
MULTIPART_BOUNDARY = "----markhandPhase1cSeedBoundary"
IDENTITY_FIXTURE_BOUNDARY = "phase1c-identity-fixture-boundary"
RESOURCE_FIXTURE_BOUNDARY = "phase1c-resource-fixture-boundary"
BETA_USER_ID = "33333333-3333-3333-3333-333333333301"
BETA_EMAIL = "phase1c-beta@poc.example"
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


def _bootstrap_beta_identity(password_hash_sql: str) -> None:
    # IDENTITY_FIXTURE_BOUNDARY: no registration API; bootstrap beta user + primary-org membership.
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
        COMMIT;
        """
    )
    user_ok = _psql(f"SELECT COUNT(*) FROM users WHERE id = '{BETA_USER_ID}' AND email = '{BETA_EMAIL}'")
    membership_ok = _psql(
        f"SELECT COUNT(*) FROM org_memberships WHERE org_id = '{ALPHA_ORG_ID}' AND user_id = '{BETA_USER_ID}' AND state = 'active'"
    )
    if user_ok != "1" or membership_ok != "1":
        raise RuntimeError("identity fixture bootstrap validation failed")


def _bootstrap_conflict_fixture(
    *,
    org_id: str,
    collection_id: str,
    document_id: str,
    version_id: str,
    conflict_id: str,
) -> None:
    # RESOURCE_FIXTURE_BOUNDARY: conflicts have no production create HTTP route.
    claim_low = str(uuid.uuid4())
    claim_high = str(uuid.uuid4())
    _psql(
        f"""
        BEGIN;
        SET LOCAL app.org_id = '{org_id}';
        INSERT INTO claims (id, org_id, document_id, version_id, field_path, normalized_value, raw_value)
        VALUES
            ('{claim_low}', '{org_id}', '{document_id}', '{version_id}', 'amount', '100', '100'),
            ('{claim_high}', '{org_id}', '{document_id}', '{version_id}', 'amount', '200', '200')
        ON CONFLICT (id) DO NOTHING;
        INSERT INTO conflicts (
            id, org_id, status, severity, conflict_type, claim_a_id, claim_b_id, first_detected_version_id
        ) VALUES (
            '{conflict_id}', '{org_id}', 'open', 'warning', 'numeric', '{claim_low}', '{claim_high}', '{version_id}'
        ) ON CONFLICT (id) DO NOTHING;
        COMMIT;
        """
    )
    ok = _psql(f"SELECT COUNT(*) FROM conflicts WHERE id = '{conflict_id}' AND org_id = '{org_id}'")
    if ok != "1":
        raise RuntimeError("conflict fixture bootstrap validation failed")


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


def _auth_me_session_id(api_base: str, access_token: str) -> str:
    status, _, body = _http(
        api_base=api_base,
        method="GET",
        path="/api/v1/auth/me",
        token=access_token,
    )
    if status != 200:
        raise RuntimeError("auth/me failed during seed")
    payload = json.loads(body)
    session_id = payload.get("sessionId") or payload.get("session_id")
    if not isinstance(session_id, str) or not session_id.strip():
        raise RuntimeError("auth/me missing sessionId")
    return session_id


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


def _issue_download_capability(
    *, api_base: str, token: str, document_id: str, version_id: str
) -> str:
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
    return project_id


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
    return session_id


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
    _bootstrap_beta_identity(password_hash)

    alpha_login = _login(api_base, alpha_email, password)
    alpha_access = alpha_login.get("accessToken") or alpha_login.get("access_token")
    alpha_refresh = alpha_login.get("refreshToken") or alpha_login.get("refresh_token")
    if not isinstance(alpha_access, str) or not isinstance(alpha_refresh, str):
        raise RuntimeError("alpha login missing token pair")

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

    beta_login = _login(api_base, BETA_EMAIL, password)
    beta_access = beta_login.get("accessToken") or beta_login.get("access_token")
    beta_refresh = beta_login.get("refreshToken") or beta_login.get("refresh_token")
    if not isinstance(beta_access, str) or not isinstance(beta_refresh, str):
        raise RuntimeError("beta login missing token pair")

    def create_collection(token: str, *, marker: str, slug_prefix: str) -> str:
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
        return collection_id

    alpha_collection_id = create_collection(alpha_access, marker=marker_alpha, slug_prefix="phase1c-alpha")
    beta_collection_id = create_collection(beta_access, marker=marker_beta, slug_prefix="phase1c-beta")

    invite_status, _, invite_raw = _http(
        api_base=api_base,
        method="POST",
        path="/api/v1/members/invites",
        token=alpha_access,
        body={"email": BETA_EMAIL, "role": "editor"},
    )
    if invite_status not in {200, 201}:
        raise RuntimeError("member invite failed")
    invite_payload = json.loads(invite_raw)
    invite = invite_payload.get("invite") or {}
    invite_id = invite.get("id")
    invite_token = invite_payload.get("token")
    if not isinstance(invite_id, str) or not isinstance(invite_token, str):
        raise RuntimeError("invite response missing id/token")

    accept_status, _, _ = _http(
        api_base=api_base,
        method="POST",
        path="/api/v1/members/invites/accept",
        token=beta_access,
        body={"token": invite_token},
    )
    if accept_status not in {200, 201}:
        raise RuntimeError("invite accept failed")

    alpha_upload = _multipart_upload(
        api_base=api_base,
        token=alpha_access,
        collection_id=alpha_collection_id,
        filename="phase1c-alpha.txt",
        content=f"{marker_alpha}\n".encode("utf-8"),
    )
    beta_upload = _multipart_upload(
        api_base=api_base,
        token=beta_access,
        collection_id=beta_collection_id,
        filename="phase1c-beta.txt",
        content=f"{marker_beta}\n".encode("utf-8"),
    )
    alpha_document_id = alpha_upload["documentId"]
    beta_document_id = beta_upload["documentId"]
    alpha_version_id = alpha_upload["versionId"]
    beta_version_id = beta_upload["versionId"]
    alpha_job_id = alpha_upload.get("jobId") or ""
    beta_job_id = beta_upload.get("jobId") or ""
    if not isinstance(alpha_job_id, str):
        alpha_job_id = ""
    if not isinstance(beta_job_id, str):
        beta_job_id = ""

    _publish_version(
        api_base=api_base,
        token=alpha_access,
        document_id=alpha_document_id,
        version_id=alpha_version_id,
    )
    _publish_version(
        api_base=api_base,
        token=beta_access,
        document_id=beta_document_id,
        version_id=beta_version_id,
    )
    alpha_download_capability = _issue_download_capability(
        api_base=api_base,
        token=alpha_access,
        document_id=alpha_document_id,
        version_id=alpha_version_id,
    )
    beta_download_capability = _issue_download_capability(
        api_base=api_base,
        token=beta_access,
        document_id=beta_document_id,
        version_id=beta_version_id,
    )
    alpha_project_id = _create_project(
        api_base=api_base, token=alpha_access, name=f"phase1c-alpha-project-{secrets.token_hex(3)}"
    )
    beta_project_id = _create_project(
        api_base=api_base, token=beta_access, name=f"phase1c-beta-project-{secrets.token_hex(3)}"
    )
    alpha_chat_session_id = _create_chat_session(
        api_base=api_base, token=alpha_access, title=f"phase1c-alpha-chat-{secrets.token_hex(3)}"
    )
    beta_chat_session_id = _create_chat_session(
        api_base=api_base, token=beta_access, title=f"phase1c-beta-chat-{secrets.token_hex(3)}"
    )

    alpha_conflict_id = str(uuid.uuid4())
    beta_conflict_id = str(uuid.uuid4())
    _bootstrap_conflict_fixture(
        org_id=ALPHA_ORG_ID,
        collection_id=alpha_collection_id,
        document_id=alpha_document_id,
        version_id=alpha_version_id,
        conflict_id=alpha_conflict_id,
    )
    _bootstrap_conflict_fixture(
        org_id=org_beta_id,
        collection_id=beta_collection_id,
        document_id=beta_document_id,
        version_id=beta_version_id,
        conflict_id=beta_conflict_id,
    )

    alpha_session_id = _auth_me_session_id(api_base, alpha_access)
    beta_session_id = _auth_me_session_id(api_base, beta_access)

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
        "alphaInviteId": invite_id,
        "betaInviteId": invite_id,
        "betaDownloadCapability": beta_download_capability,
        "alphaDownloadCapability": alpha_download_capability,
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
        "betaAccessToken": beta_access,
        "betaRefreshToken": beta_refresh,
        "alphaSessionId": alpha_session_id,
        "betaSessionId": beta_session_id,
        "betaInviteToken": invite_token,
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
