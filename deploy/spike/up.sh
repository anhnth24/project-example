#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "$0")/common.sh"

"${COMPOSE[@]}" up -d postgres qdrant minio minio-init otel mock-embedding

for _ in $(seq 1 60); do
  init_id="$("${COMPOSE[@]}" ps --all -q minio-init)"
  if [[ -n "$init_id" ]]; then
    status="$(docker inspect --format '{{.State.Status}}' "$init_id")"
    if [[ "$status" == "exited" ]]; then
      code="$(docker inspect --format '{{.State.ExitCode}}' "$init_id")"
      if [[ "$code" != "0" ]]; then
        "${COMPOSE[@]}" logs minio-init >&2
        exit 1
      fi
      break
    fi
  fi
  sleep 1
done
if [[ "${status:-}" != "exited" ]]; then
  echo "timed out waiting for spike minio-init" >&2
  "${COMPOSE[@]}" logs minio-init >&2 || true
  exit 1
fi

if [[ "${SPIKE_GPU:-0}" == "1" ]]; then
  "${COMPOSE[@]}" --profile gpu up -d vllm
fi

"$SPIKE_DIR/health.sh"
"$SPIKE_DIR/seed.sh"
python3 "$ROOT/bench/markhand_web/scripts/fingerprint_spike.py" \
  --env-file "$ENV_FILE"
