#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
COMPOSE=(docker compose -f "$ROOT/deploy/dev/compose.yml")

ENV_FILE="$ROOT/deploy/dev/.env"
if [[ -f "$ENV_FILE" ]]; then
  set -a
  # shellcheck disable=SC1090
  source "$ENV_FILE"
  set +a
fi

wait_http() {
  local url="$1"
  local name="$2"
  local attempts="${3:-30}"
  for _ in $(seq 1 "$attempts"); do
    if curl --fail --silent --show-error "$url" >/dev/null; then
      echo "healthy: $name"
      return
    fi
    sleep 1
  done
  echo "unhealthy: $name ($url)" >&2
  return 1
}

for _ in $(seq 1 60); do
  postgres_id="$("${COMPOSE[@]}" ps -q postgres)"
  if [[ -n "$postgres_id" ]] &&
    [[ "$(docker inspect --format '{{.State.Health.Status}}' "$postgres_id")" == "healthy" ]]; then
    echo "healthy: postgres"
    break
  fi
  sleep 1
done
postgres_id="$("${COMPOSE[@]}" ps -q postgres)"
if [[ -z "$postgres_id" ]] ||
  [[ "$(docker inspect --format '{{.State.Health.Status}}' "$postgres_id")" != "healthy" ]]; then
  echo "unhealthy: postgres" >&2
  exit 1
fi
wait_http "http://127.0.0.1:${MARKHAND_QDRANT_HTTP_PORT:-6333}/healthz" qdrant 90
wait_http "http://127.0.0.1:${MARKHAND_MINIO_API_PORT:-9000}/minio/health/live" minio
wait_http "http://127.0.0.1:${MARKHAND_OTEL_HEALTH_PORT:-13133}/" otel

EMBED_PORT="${MARKHAND_EMBEDDING_PORT:-8088}"
PROFILES="${COMPOSE_PROFILES:-aiteamvn}"
if [[ "$PROFILES" == *mock* ]]; then
  wait_http "http://127.0.0.1:${EMBED_PORT}/health" mock-embedding 60
else
  echo "waiting for AITeamVN embedding-cpu (first start may download model, up to ~15 min)..."
  wait_http "http://127.0.0.1:${EMBED_PORT}/health" embedding-cpu 900
fi
