#!/usr/bin/env bash
# Load deploy/dev/.env and run fileconv-server (foreground).
# Prerequisite: Compose stack up (`make dev-up`), migrations applied.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
ENV_FILE="$ROOT/deploy/dev/.env"

if [[ ! -f "$ENV_FILE" ]]; then
  echo "missing deploy/dev/.env — run: make dev-init" >&2
  exit 1
fi

# Preserve caller/CI COMPOSE_PROFILES over values in .env.
incoming_profiles="${COMPOSE_PROFILES:-}"

set -a
# shellcheck disable=SC1090
source <(sed 's/\r$//' "$ENV_FILE")
set +a

if [[ -n "$incoming_profiles" ]]; then
  export COMPOSE_PROFILES="$incoming_profiles"
fi

# Windows Make → Git Bash often omits ~/.cargo/bin even when PowerShell has it.
export PATH="${CARGO_HOME:+${CARGO_HOME}/bin:}${HOME}/.cargo/bin:${USERPROFILE:+${USERPROFILE}/.cargo/bin:}${PATH}"
if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found — install Rust 1.88 and ensure ~/.cargo/bin is on PATH" >&2
  exit 127
fi

cd "$ROOT"
exec cargo run -p fileconv-server "$@"
