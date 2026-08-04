#!/usr/bin/env bash
# Seed a second qualifying org through production HTTP APIs (Phase 1C G1C).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
API_BASE="${MARKHAND_API_BASE:-http://127.0.0.1:${MARKHAND_API_PORT:-8788}}"
EMAIL="${MARKHAND_PHASE1C_SEED_EMAIL:-admin@poc.example}"
PASSWORD="${MARKHAND_PHASE1C_SEED_PASSWORD:-${MARKHAND_O04_API_PASSWORD:-markhand-dev}}"
ORG_SLUG="${MARKHAND_PHASE1C_SECOND_ORG_SLUG:-phase1c-quiet-org}"
ORG_NAME="${MARKHAND_PHASE1C_SECOND_ORG_NAME:-Phase 1C Quiet Org}"
OUT="${MARKHAND_PHASE1C_SEED_JSON:-$ROOT/.artifacts/phase1c-multi-org-seed.json}"

export API_BASE EMAIL PASSWORD ORG_SLUG ORG_NAME OUT
mkdir -p "$(dirname "$OUT")"

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
token = body.get("accessToken") or body.get("access_token")
if not token:
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

export CREATE_RESP
python3 - <<'PY'
import json, os
from pathlib import Path

seed = {
    "apiBase": os.environ["API_BASE"],
    "primaryOrgId": "11111111-1111-1111-1111-111111111111",
    "seededSecondOrg": json.loads(os.environ["CREATE_RESP"]),
    "orgCount": 2,
    "embeddingProfile": "mock",
}
Path(os.environ["OUT"]).write_text(json.dumps(seed, indent=2, sort_keys=True) + "\n", encoding="utf-8")
print(os.environ["OUT"])
PY
