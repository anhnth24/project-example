#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SPIKE_DIR="$ROOT/deploy/spike"
ENV_FILE="${MARKHAND_SPIKE_ENV_FILE:-$SPIKE_DIR/.env}"
if [[ ! -f "$ENV_FILE" ]]; then
  ENV_FILE="$SPIKE_DIR/.env.example"
fi

nested_override="${MARKHAND_SPIKE_NESTED:-}"
set -a
# shellcheck disable=SC1090
source "$ENV_FILE"
set +a
if [[ -n "$nested_override" ]]; then
  MARKHAND_SPIKE_NESTED="$nested_override"
  export MARKHAND_SPIKE_NESTED
fi

COMPOSE=(
  docker compose
  --env-file "$ENV_FILE"
  --project-name "${MARKHAND_COMPOSE_PROJECT:-markhand-spike}"
  -f "$ROOT/deploy/dev/compose.yml"
  -f "$ROOT/deploy/compose.spike.yml"
)
if [[ "${MARKHAND_SPIKE_NESTED:-0}" == "1" ]]; then
  COMPOSE+=(-f "$ROOT/deploy/spike/compose.nested.yml")
fi
