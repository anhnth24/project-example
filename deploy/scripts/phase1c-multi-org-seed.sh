#!/usr/bin/env bash
# Seed a second qualifying org through production HTTP APIs (Phase 1C G1C).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
API_BASE="${MARKHAND_API_BASE:-http://127.0.0.1:${MARKHAND_API_PORT:-8788}}"
EMAIL="${MARKHAND_PHASE1C_SEED_EMAIL:-admin@poc.example}"
PASSWORD="${MARKHAND_PHASE1C_SEED_PASSWORD:-${MARKHAND_O04_API_PASSWORD:-markhand-dev}}"
CHALLENGE="${MARKHAND_PHASE1C_CHALLENGE:-phase1c-seed-$(python3 -c 'import secrets; print(secrets.token_hex(8))')}"
ORG_SLUG="${MARKHAND_PHASE1C_SECOND_ORG_SLUG:-phase1c-quiet-org}"
ORG_NAME="${MARKHAND_PHASE1C_SECOND_ORG_NAME:-Phase 1C Quiet Org}"
MARKER_ALPHA="${MARKHAND_PHASE1C_MARKER_ALPHA:-phase1c-marker-alpha}"
MARKER_BETA="${MARKHAND_PHASE1C_MARKER_BETA:-phase1c-marker-beta}"
STAGING="${MARKHAND_PHASE1C_SEED_STAGING:-$ROOT/.artifacts/phase1c-multi-org-seed.staging.json}"
OUT="${MARKHAND_PHASE1C_SEED_JSON:-$ROOT/.artifacts/phase1c-multi-org-seed.json}"

export API_BASE EMAIL PASSWORD CHALLENGE ORG_SLUG ORG_NAME MARKER_ALPHA MARKER_BETA STAGING OUT ROOT
mkdir -p "$(dirname "$STAGING")" "$(dirname "$OUT")"

login_body="$(EMAIL="$EMAIL" PASSWORD="$PASSWORD" python3 - <<'PY'
import json, os
print(json.dumps({"email": os.environ["EMAIL"], "password": os.environ["PASSWORD"]}))
PY
)"

login_resp="$(curl --fail-with-body -sS -X POST "$API_BASE/api/v1/auth/login" \
  -H 'content-type: application/json' \
  -d "$login_body")"

token="$(CREATE_RESP="$login_resp" python3 - <<'PY'
import json, os, sys
body = json.loads(os.environ["CREATE_RESP"])
if not isinstance(body, dict):
    sys.exit("login response must be object")
token = body.get("accessToken") or body.get("access_token")
if not isinstance(token, str) or not token.strip():
    sys.exit("login response missing access token")
print(token)
PY
)"

create_body="$(ORG_SLUG="$ORG_SLUG" ORG_NAME="$ORG_NAME" python3 - <<'PY'
import json, os
print(json.dumps({"slug": os.environ["ORG_SLUG"], "name": os.environ["ORG_NAME"]}))
PY
)"

create_resp="$(curl --fail-with-body -sS -X POST "$API_BASE/api/v1/orgs" \
  -H "authorization: Bearer $token" \
  -H 'content-type: application/json' \
  -d "$create_body")"

export CREATE_RESP ROOT
python3 - <<'PY'
import hashlib
import json
import os
import tempfile
from pathlib import Path

create = json.loads(os.environ["CREATE_RESP"])
if not isinstance(create, dict):
    raise SystemExit("create org response must be object")
org_id = create.get("id") or create.get("orgId")
slug = create.get("slug")
if not isinstance(org_id, str) or not org_id.strip():
    raise SystemExit("create org response missing id")
if not isinstance(slug, str) or not slug.strip():
    raise SystemExit("create org response missing slug")

manifest_path = Path(os.environ["ROOT"]) / "crates/server/tests/fixtures/multi-org-denial.manifest.json"
manifest_sha = hashlib.sha256(manifest_path.read_bytes()).hexdigest() if manifest_path.is_file() else ""

seed = {
    "schemaVersion": 1,
    "challenge": os.environ["CHALLENGE"],
    "environmentId": "phase1c-multi-org-poc",
    "workloadProfileId": "phase1c-multi-org",
    "embeddingProfile": "mock",
    "orgCount": 2,
    "orgAlphaId": "11111111-1111-1111-1111-111111111111",
    "orgBetaId": org_id,
    "orgBetaSlug": slug,
    "markerAlpha": os.environ["MARKER_ALPHA"],
    "markerBeta": os.environ["MARKER_BETA"],
    "manifestSha256": manifest_sha,
    "completionMarker": "PHASE1C_SEED_EOF",
}
staging = Path(os.environ["STAGING"])
final_path = Path(os.environ["OUT"])
payload = json.dumps(seed, indent=2, sort_keys=True) + "\n"
fd, tmp_name = tempfile.mkstemp(prefix=".phase1c-seed.", dir=staging.parent)
try:
    with os.fdopen(fd, "w", encoding="utf-8") as handle:
        handle.write(payload)
        handle.flush()
        os.fsync(handle.fileno())
    Path(tmp_name).replace(staging)
    fd2, tmp_final = tempfile.mkstemp(prefix=".phase1c-seed.", dir=final_path.parent)
    with os.fdopen(fd2, "w", encoding="utf-8") as handle:
        handle.write(payload)
        handle.flush()
        os.fsync(handle.fileno())
    Path(tmp_final).replace(final_path)
finally:
    if Path(tmp_name).exists() and not staging.exists():
        Path(tmp_name).unlink(missing_ok=True)
print(str(final_path))
PY
