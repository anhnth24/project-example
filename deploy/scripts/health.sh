#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
COMPOSE=(docker compose -f "$ROOT/deploy/dev/compose.yml")

wait_http() {
  local url="$1"
  local name="$2"
  for _ in $(seq 1 30); do
    if curl --fail --silent --show-error "$url" >/dev/null; then
      echo "healthy: $name"
      return
    fi
    sleep 1
  done
  echo "unhealthy: $name ($url)" >&2
  return 1
}

"${COMPOSE[@]}" exec -T postgres pg_isready -U "${MARKHAND_POSTGRES_USER:-markhand}" \
  -d "${MARKHAND_POSTGRES_DB:-markhand}" >/dev/null
wait_http "http://127.0.0.1:${MARKHAND_QDRANT_HTTP_PORT:-6333}/healthz" qdrant
wait_http "http://127.0.0.1:${MARKHAND_MINIO_API_PORT:-9000}/minio/health/live" minio
wait_http "http://127.0.0.1:${MARKHAND_OTEL_HEALTH_PORT:-13133}/" otel
wait_http "http://127.0.0.1:${MARKHAND_MOCK_EMBEDDING_PORT:-8088}/health" mock-embedding
