#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "$0")/common.sh"

wait_http() {
  local url="$1"
  local name="$2"
  local attempts="${3:-90}"
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

for _ in $(seq 1 90); do
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

wait_http "http://127.0.0.1:${MARKHAND_SPIKE_QDRANT_HTTP_PORT}/healthz" qdrant
wait_http "http://127.0.0.1:${MARKHAND_SPIKE_MINIO_API_PORT}/minio/health/live" minio
wait_http "http://127.0.0.1:${MARKHAND_SPIKE_OTEL_HEALTH_PORT}/" otel
wait_http "http://127.0.0.1:${MARKHAND_SPIKE_MOCK_EMBEDDING_PORT}/health" mock-embedding

if [[ "${SPIKE_GPU:-0}" == "1" ]]; then
  wait_http "http://127.0.0.1:${MARKHAND_SPIKE_VLLM_PORT}/health" vllm 180
fi
