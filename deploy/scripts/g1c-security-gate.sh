#!/usr/bin/env bash
# Phase 1C G1C-SEC deployed qualification orchestrator.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

OUTPUT_DIR="${1:-${MARKHAND_PHASE1C_OUTPUT_DIR:-${TMPDIR:-/tmp}/markhand-phase1c-gate}}"
shift || true

if [[ "${1:-}" == "--output-dir" ]]; then
  OUTPUT_DIR="$2"
  shift 2
fi

if [[ "${1:-}" == "--self-test" ]]; then
  exec python3 bench/markhand_web/scripts/test_run_phase1c_gate.py
fi

: "${MARKHAND_TEST_REQUIRED:=1}"
: "${MARKHAND_PHASE1C_GATE:=1}"
: "${COMPOSE_PROFILES:=mock}"
export MARKHAND_TEST_REQUIRED MARKHAND_PHASE1C_GATE COMPOSE_PROFILES

if [[ "$COMPOSE_PROFILES" != *mock* ]]; then
  echo "G1C qualifying run requires mock embedding profile (COMPOSE_PROFILES=mock)" >&2
  exit 1
fi

mkdir -p "$OUTPUT_DIR"

# POC credentials for deployed probes (deploy/.env.example defaults).
export MARKHAND_TEST_DATABASE_URL="${MARKHAND_TEST_DATABASE_URL:-postgresql://markhand:markhand_poc_change_me@127.0.0.1:54330/markhand}"
export MARKHAND_TEST_APP_DATABASE_URL="${MARKHAND_TEST_APP_DATABASE_URL:-postgresql://markhand_app:markhand_app_poc_change_me@127.0.0.1:54330/markhand}"
export MARKHAND_TEST_MINIO_ENDPOINT="${MARKHAND_TEST_MINIO_ENDPOINT:-http://127.0.0.1:9010}"
export MARKHAND_TEST_MINIO_ACCESS_KEY="${MARKHAND_TEST_MINIO_ACCESS_KEY:-markhand_root}"
export MARKHAND_TEST_MINIO_SECRET_KEY="${MARKHAND_TEST_MINIO_SECRET_KEY:-markhand_root_poc_change_me}"
export MARKHAND_TEST_MINIO_REGION="${MARKHAND_TEST_MINIO_REGION:-us-east-1}"
export MARKHAND_TEST_QDRANT_URL="${MARKHAND_TEST_QDRANT_URL:-http://127.0.0.1:6343}"
export MARKHAND_TEST_QDRANT_ADMIN_API_KEY="${MARKHAND_TEST_QDRANT_ADMIN_API_KEY:-test-operator-admin-key}"

echo "G1C security gate (MARKHAND_PHASE1C_GATE=${MARKHAND_PHASE1C_GATE})"

# Seed POC admin password when deploy/.env exists (same pattern as O04 gate).
if [[ -f deploy/.env ]]; then
  # shellcheck disable=SC1091
  set -a && source deploy/.env && set +a
fi
if [[ -n "${MARKHAND_O04_API_PASSWORD:-}" ]]; then
  hash="$(cargo run -q -p fileconv-server --bin dev-hash-password -- "$MARKHAND_O04_API_PASSWORD")"
  hash_sql="${hash//\'/\'\'}"
  docker compose -f deploy/compose.poc.yml exec -T postgres psql \
    -U "${MARKHAND_POSTGRES_USER:-markhand}" -d "${MARKHAND_POSTGRES_DB:-markhand}" \
    --set ON_ERROR_STOP=1 \
    -c "UPDATE users SET password_hash = '${hash_sql}', updated_at = now() WHERE email = 'admin@poc.example';" \
    >/dev/null
fi

bash deploy/scripts/phase1c-multi-org-seed.sh
cargo build -p fileconv-cli --no-default-features

python3 bench/markhand_web/scripts/run_phase1c_gate.py --output-dir "$OUTPUT_DIR"

exec env \
  MARKHAND_PHASE1C_GATE=1 \
  MARKHAND_PHASE1C_REPORT_PATH="$OUTPUT_DIR/phase-1c-gate.json" \
  cargo test -p fileconv-server --test e2e_phase1c_gate -- --ignored e2e_phase1c_gate --nocapture "$@"
