#!/usr/bin/env bash
# Bring up the Markhand POC compose stack (P1B-F02).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=poc-compose.sh
source "$ROOT/deploy/scripts/poc-compose.sh"
poc_compose_init

echo "building API + worker images..."
"${COMPOSE[@]}" build api worker-convert worker-index worker-embedding

echo "starting POC stack (profiles=${COMPOSE_PROFILES})..."
"${COMPOSE[@]}" up -d

echo "waiting for minio-init..."
init_status=""
for _ in $(seq 1 90); do
  init_id="$("${COMPOSE[@]}" ps --all -q minio-init || true)"
  if [[ -n "$init_id" ]]; then
    init_status="$(docker inspect --format '{{.State.Status}}' "$init_id")"
    if [[ "$init_status" == "exited" ]]; then
      init_code="$(docker inspect --format '{{.State.ExitCode}}' "$init_id")"
      [[ "$init_code" == "0" ]] || {
        echo "minio-init failed with exit code $init_code" >&2
        "${COMPOSE[@]}" logs minio-init >&2 || true
        exit 1
      }
      break
    fi
  fi
  sleep 1
done

[[ "$init_status" == "exited" ]] || {
  echo "timed out waiting for minio-init" >&2
  "${COMPOSE[@]}" ps >&2 || true
  exit 1
}

"$ROOT/deploy/scripts/poc-health.sh"
echo "POC stack is up"
