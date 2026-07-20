#!/usr/bin/env bash
# Print copy-paste dev defaults (env, logins, UUIDs, worker commands).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
ENV_FILE="$ROOT/deploy/dev/.env"

if [[ -f "$ENV_FILE" ]]; then
  set -a
  # shellcheck disable=SC1090
  source "$ENV_FILE"
  set +a
fi

PASSWORD="${MARKHAND_DEV_PASSWORD:-markhand-dev}"
BIND="${MARKHAND_BIND_ADDR:-127.0.0.1:8787}"
API="http://${BIND}/api/v1"
SIGNATURE="${MARKHAND_INDEX_SIGNATURE:-ca03085c08f4c01d391ac973192815c944892f6e74b52e7bf4e1f135f65ae97c}"

cat <<EOF
╔══════════════════════════════════════════════════════════════════╗
║  Markhand local dev — defaults (dev-only, never use in prod)   ║
╚══════════════════════════════════════════════════════════════════╝

Stack
  make dev-up / make dev-health
  PostgreSQL      127.0.0.1:${MARKHAND_POSTGRES_PORT:-54329}
  Qdrant          http://127.0.0.1:${MARKHAND_QDRANT_HTTP_PORT:-6333}
  MinIO API       http://127.0.0.1:${MARKHAND_MINIO_API_PORT:-9000}
  MinIO console   http://127.0.0.1:${MARKHAND_MINIO_CONSOLE_PORT:-9001}
  Embedding (AITeamVN CPU)  http://127.0.0.1:${MARKHAND_EMBEDDING_PORT:-8088}/v1/embeddings
  COMPOSE_PROFILES=${COMPOSE_PROFILES:-aiteamvn}  (mock: COMPOSE_PROFILES=mock for CI)

API
  Base URL        $API
  Health          curl --fail $API/health/ready

Auth (enabled in deploy/dev/.env.example)
  Password        $PASSWORD
  admin@poc.example     role admin (migration 0011)
  owner@example.test    role owner (seed-poc-org)

POC UUIDs (migration 0011)
  Org             11111111-1111-1111-1111-111111111111
  Admin user      22222222-2222-2222-2222-222222222201
  Collection      55555555-5555-5555-5555-555555555501  (slug: poc-library)

Embedding (AITeamVN CPU — chậm lần đầu, cùng runtime on-prem CPU)
  MARKHAND_EMBEDDING_BASE_URL=${MARKHAND_EMBEDDING_BASE_URL:-http://127.0.0.1:8088/v1}
  MARKHAND_EMBEDDING_MODEL=${MARKHAND_EMBEDDING_MODEL:-AITeamVN/Vietnamese_Embedding}
  MARKHAND_EMBEDDING_DIMENSIONS=${MARKHAND_EMBEDDING_DIMENSIONS:-1024}
  MARKHAND_INDEX_SIGNATURE=$SIGNATURE

Load env
  set -a && source deploy/dev/.env && set +a
  set -a && source deploy/dev/worker.env && set +a

Server
  deploy/scripts/bootstrap-server-role.sh   # once per fresh PG volume
  cargo run -p fileconv-server

Workers (Linux/WSL — separate terminals)
  export MARKHAND_WORKER_KIND=convert   # + MARKHAND_CONVERTER_ARGV_JSON
  export MARKHAND_WORKER_KIND=index
  export MARKHAND_WORKER_KIND=embedding
  cargo run --release -p fileconv-server --bin fileconv-worker

Quick login (after seed-dev-all.sh)
  curl -sS -X POST $API/auth/login \\
    -H 'Content-Type: application/json' \\
    -d '{"email":"admin@poc.example","password":"$PASSWORD"}'

Upload collectionId
  55555555-5555-5555-5555-555555555501

Prefetch model (optional):
  deploy/scripts/download-aiteamvn-embedding.sh

EOF
