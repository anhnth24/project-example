#!/usr/bin/env bash
# P1B-O04 live vertical-slice / security release suite against compose.poc.yml.
# Fail-closed: refuses human environments; never silently skips; never reuses untagged stacks.
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

# Dedicated E2E defaults — never fall back to human markhand / markhand-poc / markhand-documents.
export MARKHAND_COMPOSE_PROJECT="${MARKHAND_COMPOSE_PROJECT:-markhand-e2e}"
export MARKHAND_POSTGRES_DB="${MARKHAND_POSTGRES_DB:-markhand_e2e}"
export MARKHAND_MINIO_BUCKET="${MARKHAND_MINIO_BUCKET:-markhand-e2e-documents}"
export MARKHAND_E2E_STACK_TAG="${MARKHAND_E2E_STACK_TAG:-test}"
export MARKHAND_POSTGRES_USER="${MARKHAND_POSTGRES_USER:-markhand_e2e}"

# Optional: load deploy/.env but refuse if it overrides to human names without e2e/test tags.
if [[ -f "$ROOT/deploy/.env" ]]; then
  set -a
  # shellcheck disable=SC1091
  source "$ROOT/deploy/.env"
  set +a
fi

# Re-assert E2E defaults if .env wiped tags (human stack must never be reused).
project="${MARKHAND_COMPOSE_PROJECT:-}"
db="${MARKHAND_POSTGRES_DB:-}"
bucket="${MARKHAND_MINIO_BUCKET:-}"
tag="${MARKHAND_E2E_STACK_TAG:-}"

[[ "$project" =~ [Ee]2[Ee]|[Tt][Ee][Ss][Tt] ]] \
  || die "MARKHAND_COMPOSE_PROJECT must contain e2e/test (got '$project'); refusing untagged stack"
[[ "$db" =~ [Ee]2[Ee]|[Tt][Ee][Ss][Tt] ]] \
  || die "MARKHAND_POSTGRES_DB must contain e2e/test (got '$db'); refusing untagged stack"
[[ "$bucket" =~ [Ee]2[Ee]|[Tt][Ee][Ss][Tt] ]] \
  || die "MARKHAND_MINIO_BUCKET must contain e2e/test (got '$bucket'); refusing untagged stack"
[[ "$tag" == "test" ]] || die "MARKHAND_E2E_STACK_TAG must be 'test'"
[[ "$project" != "markhand" && "$project" != "markhand-poc" ]] \
  || die "refusing untagged/human compose project '$project'"
[[ "$db" != "markhand" ]] || die "refusing human postgres db 'markhand'"
[[ "$bucket" != "markhand-documents" ]] || die "refusing human minio bucket"

require_cmd docker
require_cmd python3
require_cmd cargo
docker info >/dev/null 2>&1 || die "Docker engine not available"

poc_compose_init

CLEANUP_FAILED=0
cleanup_live() {
  local rc=$?
  # Verify tagged isolation BEFORE any cleanup mutation.
  if ! [[ "${MARKHAND_COMPOSE_PROJECT:-}" =~ [Ee]2[Ee]|[Tt][Ee][Ss][Tt] ]] \
    || ! [[ "${MARKHAND_POSTGRES_DB:-}" =~ [Ee]2[Ee]|[Tt][Ee][Ss][Tt] ]] \
    || ! [[ "${MARKHAND_MINIO_BUCKET:-}" =~ [Ee]2[Ee]|[Tt][Ee][Ss][Tt] ]] \
    || [[ "${MARKHAND_E2E_STACK_TAG:-}" != "test" ]]; then
    echo "FATAL: cleanup high/critical — refusing restore on untagged stack" >&2
    exit 1
  fi
  if [[ -n "${COMPOSE[*]:-}" ]]; then
    if ! "${COMPOSE[@]}" start worker-convert worker-index qdrant api >/dev/null 2>&1; then
      echo "FATAL: cleanup high/critical — service restore failed" >&2
      CLEANUP_FAILED=1
    fi
    # psql variables — never interpolate identities into SQL text.
    if ! "${COMPOSE[@]}" exec -T postgres psql \
      -U "${MARKHAND_POSTGRES_USER}" \
      -d "${MARKHAND_POSTGRES_DB}" \
      --set ON_ERROR_STOP=1 \
      -v email_a=admin@poc.example \
      -v email_b=editor-e2e@poc.example \
      -v email_c=viewer-e2e@poc.example \
      -v email_d=owner@org-b.example \
      -c "UPDATE users SET disabled_at = NULL WHERE email IN (:'email_a', :'email_b', :'email_c', :'email_d');" \
      >/dev/null 2>&1; then
      echo "FATAL: cleanup high/critical — user restore failed" >&2
      CLEANUP_FAILED=1
    fi
  fi
  if [[ "$CLEANUP_FAILED" -ne 0 ]]; then
    exit 1
  fi
  exit "$rc"
}
trap cleanup_live EXIT

# Bring stack up (idempotent) on the tagged e2e project only.
"$ROOT/deploy/scripts/poc-up.sh"
"$ROOT/deploy/scripts/poc-health.sh"

# Seed accounts (admin + editor/viewer + foreign org/collection — no documents via SQL).
"$ROOT/deploy/scripts/seed-poc-e2e.sh"

# Pass compose argv to the Python runner as a JSON array.
MARKHAND_E2E_COMPOSE_JSON="$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "${COMPOSE[@]}")"
export MARKHAND_E2E_COMPOSE_JSON
export MARKHAND_E2E_PASSWORD="${MARKHAND_E2E_PASSWORD:-${MARKHAND_DEV_PASSWORD:-markhand-e2e}}"
export MARKHAND_E2E_ADMIN_EMAIL="${MARKHAND_E2E_ADMIN_EMAIL:-admin@poc.example}"
export MARKHAND_E2E_VIEWER_EMAIL="${MARKHAND_E2E_VIEWER_EMAIL:-viewer-e2e@poc.example}"
export MARKHAND_E2E_FOREIGN_EMAIL="${MARKHAND_E2E_FOREIGN_EMAIL:-owner@org-b.example}"
export MARKHAND_E2E_COLLECTION_ID="${MARKHAND_E2E_COLLECTION_ID:-55555555-5555-5555-5555-555555555501}"
export MARKHAND_E2E_FOREIGN_COLLECTION_ID="${MARKHAND_E2E_FOREIGN_COLLECTION_ID:-56565656-5656-4565-8565-565656565601}"
export MARKHAND_E2E_ORG_ID="${MARKHAND_E2E_ORG_ID:-11111111-1111-1111-1111-111111111111}"
export MARKHAND_E2E_ADMIN_USER_ID="${MARKHAND_E2E_ADMIN_USER_ID:-22222222-2222-2222-2222-222222222201}"
export MARKHAND_E2E_BASE_URL="${MARKHAND_E2E_BASE_URL:-http://127.0.0.1:${MARKHAND_API_PORT:-8788}}"
# Foreign document/version IDs are established by the live runner via public API only.
unset MARKHAND_E2E_FOREIGN_DOCUMENT_ID || true
unset MARKHAND_E2E_FOREIGN_VERSION_ID || true

python3 "$ROOT/crates/server/tests/e2e/fixtures/generate.py" --check
python3 "$ROOT/crates/server/tests/e2e/scripts/run_live.py"

echo "P1B-O04 live E2E completed"
