#!/usr/bin/env bash
# Start POC stack + observability overlay with correct project-directory binds.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
export REPO_ROOT

ENV_FILE="${ENV_FILE:-$REPO_ROOT/deploy/.env}"
if [[ ! -f "$ENV_FILE" ]]; then
  cp "$REPO_ROOT/deploy/.env.example" "$ENV_FILE"
  echo "created $ENV_FILE from .env.example"
fi

# shellcheck disable=SC1090
set -a && source "$ENV_FILE" && set +a
export COMPOSE_PROFILES="${COMPOSE_PROFILES:-mock}"
export REPO_ROOT

exec docker compose \
  --project-directory "$REPO_ROOT" \
  --env-file "$ENV_FILE" \
  -f "$REPO_ROOT/deploy/compose.poc.yml" \
  -f "$REPO_ROOT/deploy/observability/compose.observability.yml" \
  "$@"
