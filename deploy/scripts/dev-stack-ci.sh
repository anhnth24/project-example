#!/usr/bin/env bash
# CI dev-stack smoke with a tiered profile to avoid duplicating Rust compile work.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MODE="${DEV_STACK_MODE:-full}"
RUST_RAN_SERVER="${DEV_STACK_RUST_SERVER:-false}"

cd "$ROOT"
trap 'make dev-down || true; make spike-down || true' EXIT

docker compose -f deploy/dev/compose.yml config >/dev/null
python3 scripts/validate_spike.py --config-only
make dev-up
make dev-health

run_server_smoke=true
if [[ "$RUST_RAN_SERVER" == "true" ]]; then
  echo "skipping dev-server-smoke: Rust job already validated fileconv-server"
  run_server_smoke=false
fi

if [[ "$MODE" == "lite" ]]; then
  run_server_smoke=false
fi

if [[ "$run_server_smoke" == "true" ]]; then
  make dev-server-smoke
fi

make dev-down

if [[ "$MODE" == "full" ]]; then
  make spike-up
  make spike-lifecycle
  make spike-health
  make check-spike
  make spike-down
fi

echo "dev-stack CI profile '${MODE}' passed"
