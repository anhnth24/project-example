#!/usr/bin/env bash
# CI dev-stack smoke with a tiered profile to avoid duplicating Rust compile work.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
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

# Real-deployment half of P2-15 (web-e2e-real.sh): reuses this same dev-up
# stack while it's still up, so it runs before dev-down, not after. Gated on
# MODE == full only, same as spike-* below — it always needs its own
# fileconv-server + web build regardless of DEV_STACK_RUST_SERVER (unlike
# dev-server-smoke's skip above, this is new coverage the Rust job's own
# tests don't provide, not a duplicate of them).
if [[ "$MODE" == "full" ]]; then
  bash deploy/scripts/web-e2e-real.sh
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
