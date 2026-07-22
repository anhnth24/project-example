#!/usr/bin/env bash
# P1B-O04 live vertical-slice / security release suite against compose.poc.yml.
# Fail-closed: refuses human environments; never silently skips.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=poc-compose.sh
source "$ROOT/deploy/scripts/poc-compose.sh"

CONFIRM_PHRASE="i-understand-this-mutates-only-tagged-test-stacks"

die() { echo "FATAL: $*" >&2; exit 1; }

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "missing command: $1"
}

echo "== P1B-O04 live E2E =="

# --- Safety gates first (mirror harness/confirm.py); never mutate human envs ---
[[ "${MARKHAND_E2E_CONFIRM:-}" == "$CONFIRM_PHRASE" ]] \
  || die "set MARKHAND_E2E_CONFIRM=$CONFIRM_PHRASE"

# Load deploy/.env so project/db/bucket tags can be checked before Docker work.
if [[ ! -f "$ROOT/deploy/.env" ]]; then
  cp "$ROOT/deploy/.env.example" "$ROOT/deploy/.env"
  echo "created deploy/.env from .env.example"
fi
set -a
# shellcheck disable=SC1091
source "$ROOT/deploy/.env"
set +a

project="${MARKHAND_COMPOSE_PROJECT:-}"
db="${MARKHAND_POSTGRES_DB:-}"
bucket="${MARKHAND_MINIO_BUCKET:-}"
tag="${MARKHAND_E2E_STACK_TAG:-}"

[[ "$project" =~ [Ee]2[Ee]|[Tt][Ee][Ss][Tt] ]] \
  || die "MARKHAND_COMPOSE_PROJECT must contain e2e/test (got '$project')"
[[ "$db" =~ [Ee]2[Ee]|[Tt][Ee][Ss][Tt] ]] \
  || die "MARKHAND_POSTGRES_DB must contain e2e/test (got '$db')"
[[ "$bucket" =~ [Ee]2[Ee]|[Tt][Ee][Ss][Tt] ]] \
  || die "MARKHAND_MINIO_BUCKET must contain e2e/test (got '$bucket')"
[[ "$tag" == "test" ]] || die "MARKHAND_E2E_STACK_TAG must be 'test'"

require_cmd docker
require_cmd python3
require_cmd cargo
docker info >/dev/null 2>&1 || die "Docker engine not available"

poc_compose_init

# Bring stack up (idempotent).
"$ROOT/deploy/scripts/poc-up.sh"
"$ROOT/deploy/scripts/poc-health.sh"

# Seed accounts (admin + editor/viewer + foreign org).
"$ROOT/deploy/scripts/seed-poc-e2e.sh"

# Pass compose argv to the Python runner as a JSON array.
MARKHAND_E2E_COMPOSE_JSON="$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "${COMPOSE[@]}")"
export MARKHAND_E2E_COMPOSE_JSON
export MARKHAND_E2E_PASSWORD="${MARKHAND_E2E_PASSWORD:-${MARKHAND_DEV_PASSWORD:-markhand-e2e}}"
export MARKHAND_E2E_ADMIN_EMAIL="${MARKHAND_E2E_ADMIN_EMAIL:-admin@poc.example}"
export MARKHAND_E2E_VIEWER_EMAIL="${MARKHAND_E2E_VIEWER_EMAIL:-viewer-e2e@poc.example}"
export MARKHAND_E2E_FOREIGN_EMAIL="${MARKHAND_E2E_FOREIGN_EMAIL:-owner@org-b.example}"
export MARKHAND_E2E_COLLECTION_ID="${MARKHAND_E2E_COLLECTION_ID:-55555555-5555-5555-5555-555555555501}"
export MARKHAND_E2E_ORG_ID="${MARKHAND_E2E_ORG_ID:-11111111-1111-1111-1111-111111111111}"
export MARKHAND_E2E_ADMIN_USER_ID="${MARKHAND_E2E_ADMIN_USER_ID:-22222222-2222-2222-2222-222222222201}"
export MARKHAND_E2E_BASE_URL="${MARKHAND_E2E_BASE_URL:-http://127.0.0.1:${MARKHAND_API_PORT:-8788}}"

python3 "$ROOT/crates/server/tests/e2e/fixtures/generate.py" --check
python3 "$ROOT/crates/server/tests/e2e/scripts/run_live.py"

echo "P1B-O04 live E2E completed"