#!/usr/bin/env bash
# Real-deployment half of P2-15: build the web SPA, boot fileconv-server
# against the already-up dev Compose stack (Postgres/Qdrant/MinIO/embedding),
# and run the Playwright `real` project against it. Smoke scope only — see
# web/e2e-real/support.ts for what this suite covers and why.
#
# Pattern follows server-smoke.sh (init env -> source .env -> bootstrap ->
# migrate -> qdrant-init -> run fileconv-server -> poll /health/ready -> seed)
# but does not tear the server down until Playwright has run against it, and
# additionally builds + serves the SPA from the same process (see
# deploy/README.md's "Web SPA static serving (P2-16)" section).
#
# Prerequisites this script assumes the caller already set up (mirrors
# server-smoke.sh's own assumptions about a ready Rust toolchain):
#   - the dev Compose stack is up (`make dev-up` / dev-stack-ci.sh)
#   - Node/pnpm are on PATH and `pnpm install --frozen-lockfile` has run
#   - Playwright's Chromium is installed (`pnpm --dir web exec playwright
#     install --with-deps chromium`)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
ENV_FILE="$ROOT/deploy/dev/.env"
WEB_DIR="$ROOT/web"

# Mock signature / dims must match deploy/dev/.env.example mock block and mock-embedding.py.
MOCK_INDEX_SIGNATURE="0f59a26d542340c3c2c062a227417e47f9303c2db67569cf9031fe4707e44bf0"

created_env=false
if [[ ! -f "$ENV_FILE" ]]; then
  "$ROOT/deploy/scripts/init-dev-env.sh"
  created_env=true
fi

# CI sets COMPOSE_PROFILES=mock; do not let .env clobber it.
incoming_profiles="${COMPOSE_PROFILES:-}"

set -a
# shellcheck disable=SC1090
source "$ENV_FILE"
set +a

if [[ -n "$incoming_profiles" ]]; then
  export COMPOSE_PROFILES="$incoming_profiles"
fi

if [[ "${COMPOSE_PROFILES:-}" == *mock* ]]; then
  export MARKHAND_EMBEDDING_MODEL=markhand-mock
  export MARKHAND_EMBEDDING_REVISION=dev-local
  export MARKHAND_EMBEDDING_DIMENSIONS=8
  export MARKHAND_EMBEDDING_API_KEY=dev-mock-key
  export MARKHAND_INDEX_SIGNATURE="$MOCK_INDEX_SIGNATURE"
  export MARKHAND_MOCK_EMBEDDING_DIMENSIONS=8
fi

"$ROOT/deploy/scripts/bootstrap-server-role.sh"
"$ROOT/deploy/scripts/migrate.sh"
python3 "$ROOT/deploy/poc/qdrant-init.py"

# Build the SPA before the server starts: resolve_web_dist_dir() is checked
# once at router bootstrap, so a `web/dist` that appears after the server is
# already running would never be picked up.
pnpm --dir "$WEB_DIR" build
export MARKHAND_WEB_DIST_DIR="$WEB_DIR/dist"

log_file="$(mktemp)"
cargo build -p fileconv-server
"$ROOT/target/debug/fileconv-server" >"$log_file" 2>&1 &
server_pid=$!
cleanup() {
  kill "$server_pid" 2>/dev/null || true
  wait "$server_pid" 2>/dev/null || true
  rm -f "$log_file"
  if [[ "$created_env" == true ]]; then
    rm -f "$ENV_FILE"
  fi
}
trap cleanup EXIT

bind_addr="${MARKHAND_BIND_ADDR:-127.0.0.1:8787}"
healthy=false
for _ in $(seq 1 60); do
  if curl --fail --silent --show-error \
    "http://${bind_addr}/api/v1/health/ready" >/dev/null; then
    healthy=true
    break
  fi
  if ! kill -0 "$server_pid" 2>/dev/null; then
    cat "$log_file" >&2
    exit 1
  fi
  sleep 1
done

if [[ "$healthy" != true ]]; then
  cat "$log_file" >&2
  echo "unhealthy: fileconv-server" >&2
  exit 1
fi

"$ROOT/deploy/scripts/seed-dev-all.sh" --skip-init
echo "healthy: fileconv-server (serving web/dist from $MARKHAND_WEB_DIST_DIR)"

MARKHAND_E2E_REAL=1 \
  MARKHAND_E2E_REAL_BASE_URL="http://${bind_addr}" \
  pnpm --dir "$WEB_DIR" exec playwright test --project=real
