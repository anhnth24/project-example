#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT/deploy/dev"
if [[ -f .env ]]; then
  set -a
  # shellcheck disable=SC1091
  source .env
  set +a
fi
export COMPOSE_PROFILES="${COMPOSE_PROFILES:-aiteamvn}"
docker compose up -d

for _ in $(seq 1 30); do
  init_id="$(docker compose ps --all -q minio-init)"
  if [[ -n "$init_id" ]]; then
    init_status="$(docker inspect --format '{{.State.Status}}' "$init_id")"
    if [[ "$init_status" == "exited" ]]; then
      init_code="$(docker inspect --format '{{.State.ExitCode}}' "$init_id")"
      [[ "$init_code" == "0" ]] || {
        echo "minio-init failed with exit code $init_code" >&2
        docker compose logs minio-init >&2 || true
        exit 1
      }
      break
    fi
  fi
  sleep 1
done

[[ "${init_status:-}" == "exited" ]] || {
  echo "timed out waiting for minio-init" >&2
  docker compose ps >&2 || true
  docker compose logs >&2 || true
  exit 1
}
if ! "$ROOT/deploy/scripts/health.sh"; then
  docker compose ps >&2 || true
  docker compose logs >&2 || true
  exit 1
fi
"$ROOT/deploy/scripts/seed.sh"
