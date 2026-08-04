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
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "bench/markhand_web/scripts"))

from phase1c_deployed_probes import (  # noqa: E402
    build_public_seed_evidence,
    canonical_denial_manifest_sha256,
)

COMPOSE_FILE = ROOT / "deploy/compose.poc.yml"
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
) -> tuple[int, dict[str, str], str]:
    headers = {"content-type": "application/json"}
    if token:
        headers["authorization"] = f"Bearer {token}"
    data = json.dumps(body).encode("utf-8") if body is not None else None
    request = urllib.request.Request(f"{api_base.rstrip('/')}{path}", data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            raw = response.read().decode("utf-8", errors="replace")
            return response.status, dict(response.headers), raw
    except urllib.error.HTTPError as error:
        raw = error.read().decode("utf-8", errors="replace")
        return error.code, dict(error.headers), raw


def _psql(sql: str) -> None:
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
        "-c",
        sql,
    ]
    proc = subprocess.run(cmd, capture_output=True, text=True, check=False)
    if proc.returncode != 0:
        raise RuntimeError("poc db bootstrap failed")


def _bootstrap_beta_identity(password_hash_sql: str) -> None:
    _psql(
        f"""
        INSERT INTO users (id, email, display_name, password_hash)
        VALUES ('{BETA_USER_ID}', '{BETA_EMAIL}', 'Phase 1C Beta', '{password_hash_sql}')
        ON CONFLICT (id) DO UPDATE SET email = EXCLUDED.email, password_hash = EXCLUDED.password_hash;
        """
    )


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

    alpha_session_id = str(alpha_login.get("sessionId") or alpha_login.get("session_id") or secrets.token_hex(16))
    beta_session_id = str(beta_login.get("sessionId") or beta_login.get("session_id") or secrets.token_hex(16))

    seed_raw: dict[str, Any] = {
        "schemaVersion": 1,
        "challenge": challenge,
        "sourceRevision": source_revision,
        "manifestSha256": canonical_denial_manifest_sha256(),
        "orgAlphaId": ALPHA_ORG_ID,
        "orgBetaId": org_beta_id,
        "alphaUserId": ALPHA_USER_ID,
        "betaUserId": BETA_USER_ID,
        "markerAlpha": marker_alpha,
        "markerBeta": marker_beta,
        "alphaCollectionId": alpha_collection_id,
        "betaCollectionId": beta_collection_id,
        "alphaDocumentId": "cccccccc-cccc-cccc-cccc-cccccccccccc",
        "betaDocumentId": "dddddddd-dddd-dddd-dddd-dddddddddddd",
        "alphaJobId": "eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee",
        "betaJobId": "ffffffff-ffff-ffff-ffff-ffffffffffff",
        "alphaVersionId": "12121212-1212-1212-1212-121212121212",
        "betaVersionId": "13131313-1313-1313-1313-131313131313",
        "alphaChatSessionId": "14141414-1414-1414-1414-141414141414",
        "betaChatSessionId": "15151515-1515-1515-1515-151515151515",
        "alphaProjectId": "16161616-1616-1616-1616-161616161616",
        "betaProjectId": "17171717-1717-1717-1717-171717171717",
        "alphaConflictId": "18181818-1818-1818-1818-181818181818",
        "betaConflictId": "19191919-1919-1919-1919-191919191919",
        "betaMemberUserId": BETA_USER_ID,
        "alphaInviteId": invite_id,
        "betaInviteId": invite_id,
        "betaInviteAcceptToken": _sha256_text(invite_token),
        "betaDownloadCapability": f"cap-{_sha256_text(invite_token)[:16]}",
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
    try:
        return run_seed()
    except Exception as error:
        print(f"seed failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
