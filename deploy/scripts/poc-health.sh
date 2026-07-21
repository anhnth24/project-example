#!/usr/bin/env bash
# Health checks for the Markhand POC stack (host loopback ports + worker state).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
COMPOSE_FILE="$ROOT/deploy/compose.poc.yml"
ENV_FILE="$ROOT/deploy/.env"

if [[ -f "$ENV_FILE" ]]; then
  set -a
  # shellcheck disable=SC1090
  source "$ENV_FILE"
  set +a
fi

COMPOSE=(docker compose --env-file "$ENV_FILE" -f "$COMPOSE_FILE")
PROFILES="${COMPOSE_PROFILES:-mock}"

wait_http() {
  local url="$1"
  local name="$2"
  local attempts="${3:-60}"
  for _ in $(seq 1 "$attempts"); do
    if curl --fail --silent --show-error "$url" >/dev/null; then
      echo "healthy: $name"
      return 0
    fi
    sleep 1
  done
  echo "unhealthy: $name ($url)" >&2
  return 1
}

require_running() {
  local service="$1"
  local id
  id="$("${COMPOSE[@]}" ps -q "$service" || true)"
  if [[ -z "$id" ]]; then
    echo "unhealthy: $service (not running)" >&2
    return 1
  fi
  local status
  status="$(docker inspect --format '{{.State.Status}}' "$id")"
  if [[ "$status" != "running" ]]; then
    echo "unhealthy: $service (state=$status)" >&2
    return 1
  fi
  local health
  health="$(docker inspect --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}none{{end}}' "$id")"
  if [[ "$health" != "none" && "$health" != "healthy" ]]; then
    echo "unhealthy: $service (health=$health)" >&2
    return 1
  fi
  echo "healthy: $service (running${health:+, health=$health})"
}

for _ in $(seq 1 60); do
  postgres_id="$("${COMPOSE[@]}" ps -q postgres || true)"
  if [[ -n "$postgres_id" ]] &&
    [[ "$(docker inspect --format '{{.State.Health.Status}}' "$postgres_id")" == "healthy" ]]; then
    echo "healthy: postgres"
    break
  fi
  sleep 1
done
postgres_id="$("${COMPOSE[@]}" ps -q postgres || true)"
if [[ -z "$postgres_id" ]] ||
  [[ "$(docker inspect --format '{{.State.Health.Status}}' "$postgres_id")" != "healthy" ]]; then
  echo "unhealthy: postgres" >&2
  exit 1
fi

wait_http "http://127.0.0.1:${MARKHAND_QDRANT_HTTP_PORT:-6343}/healthz" qdrant 90
wait_http "http://127.0.0.1:${MARKHAND_MINIO_API_PORT:-9010}/minio/health/live" minio 60

EMBED_PORT="${MARKHAND_EMBEDDING_PORT:-8090}"
if [[ "$PROFILES" == *aiteamvn* ]]; then
  echo "waiting for AITeamVN embedding-cpu (first start may download model)..."
  wait_http "http://127.0.0.1:${EMBED_PORT}/health" embedding-cpu 900
else
  wait_http "http://127.0.0.1:${EMBED_PORT}/health" mock-embedding 60
fi

wait_http \
  "http://127.0.0.1:${MARKHAND_API_PORT:-8788}/api/v1/health/live" \
  api-live \
  90
wait_http \
  "http://127.0.0.1:${MARKHAND_API_PORT:-8788}/api/v1/health/ready" \
  api-ready \
  90

require_running worker-convert
require_running worker-index
require_running worker-embedding

echo "POC health OK"
