#!/usr/bin/env bash
# Real-deployment half of P2-15: build the web SPA, boot fileconv-server plus
# convert/index/embedding workers against the already-up dev Compose stack
# (Postgres/Qdrant/MinIO/embedding), and run the Playwright `real` project
# against it. Smoke scope — see web/e2e-real/support.ts for coverage.
#
# Pattern follows server-smoke.sh (init env -> source .env -> bootstrap ->
# migrate -> qdrant-init -> run fileconv-server + workers -> poll /health/ready
# -> seed) but does not tear processes down until Playwright has run, and
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
REDACT="$ROOT/deploy/scripts/redact_secrets.py"

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

export MARKHAND_WORKER_DATABASE_URL="postgres://${MARKHAND_WORKER_DB_USER:-markhand_worker}:${MARKHAND_WORKER_DB_PASSWORD:-markhand_worker_dev_only}@127.0.0.1:${MARKHAND_POSTGRES_PORT:-54329}/${MARKHAND_POSTGRES_DB:-markhand}"
export MARKHAND_CONVERTER_ARGV_JSON="[\"${ROOT}/target/debug/fileconv\",\"one\",\"{input}\"]"
server_log="$(mktemp)"
convert_log="$(mktemp)"
index_log="$(mktemp)"
embedding_log="$(mktemp)"
worker_pids=()

dump_redacted_logs() {
  local reason="${1:-failure}"
  echo "=== web-e2e-real: ${reason} ===" >&2
  local entries=(
    "server:$server_log"
    "convert:$convert_log"
    "index:$index_log"
    "embedding:$embedding_log"
  )
  local entry label path
  for entry in "${entries[@]}"; do
    label="${entry%%:*}"
    path="${entry#*:}"
    echo "--- ${label} ---" >&2
    if [[ -f "$path" ]]; then
      python3 "$REDACT" --allow-residual "$path" >&2 || cat "$path" >&2
    else
      echo "(no log file)" >&2
    fi
  done
}

workers_alive() {
  local worker_pid
  for worker_pid in "${worker_pids[@]}"; do
    if ! kill -0 "$worker_pid" 2>/dev/null; then
      dump_redacted_logs "worker process ${worker_pid} exited before Playwright"
      return 1
    fi
  done
  return 0
}

start_worker() {
  local kind="$1"
  local worker_id="$2"
  local worker_log="$3"
  local converter_argv="${4:-}"

  (
    unset MARKHAND_AUTH_ISSUER MARKHAND_AUTH_AUDIENCE MARKHAND_AUTH_SIGNING_KEY MARKHAND_AUTH_KID
    unset MARKHAND_DATABASE_URL MARKHAND_MIGRATOR_DATABASE_URL

    export MARKHAND_WORKER_DATABASE_URL
    export MARKHAND_WORKER_ORG_ID="${MARKHAND_WORKER_ORG_ID:-11111111-1111-1111-1111-111111111111}"
    export MARKHAND_WORKER_USER_ID="${MARKHAND_WORKER_USER_ID:-22222222-2222-2222-2222-222222222201}"
    case "$kind" in
      convert) export MARKHAND_WORKER_KIND=convert ;;
      index) export MARKHAND_WORKER_KIND=index ;;
      embedding) export MARKHAND_WORKER_KIND=embedding ;;
      *) echo "unknown worker kind: $kind" >&2; exit 1 ;;
    esac
    export MARKHAND_WORKER_ID="$worker_id"
    if [[ -n "$converter_argv" ]]; then
      export MARKHAND_CONVERTER_ARGV_JSON="$converter_argv"
    fi

    exec "$ROOT/target/debug/fileconv-worker" >"$worker_log" 2>&1
  ) &
  worker_pids+=("$!")
}

cleanup() {
  kill "$server_pid" 2>/dev/null || true
  local worker_pid
  for worker_pid in "${worker_pids[@]}"; do
    kill "$worker_pid" 2>/dev/null || true
  done
  wait "$server_pid" 2>/dev/null || true
  for worker_pid in "${worker_pids[@]}"; do
    wait "$worker_pid" 2>/dev/null || true
  done
  rm -f "$server_log" "$convert_log" "$index_log" "$embedding_log"
  if [[ "$created_env" == true ]]; then
    rm -f "$ENV_FILE"
  fi
}
trap cleanup EXIT

cargo build -p fileconv-server
cargo build -p fileconv-cli --bin fileconv

start_worker convert e2e-real-convert-1 "$convert_log" "$MARKHAND_CONVERTER_ARGV_JSON"
start_worker index e2e-real-index-1 "$index_log"
start_worker embedding e2e-real-embedding-1 "$embedding_log"

"$ROOT/target/debug/fileconv-server" >"$server_log" 2>&1 &
server_pid=$!

bind_addr="${MARKHAND_BIND_ADDR:-127.0.0.1:8787}"
healthy=false
for _ in $(seq 1 60); do
  if curl --fail --silent --show-error \
    "http://${bind_addr}/api/v1/health/ready" >/dev/null; then
    healthy=true
    break
  fi
  if ! kill -0 "$server_pid" 2>/dev/null; then
    dump_redacted_logs "fileconv-server exited during readiness"
    exit 1
  fi
  if ! workers_alive; then
    exit 1
  fi
  sleep 1
done

if [[ "$healthy" != true ]]; then
  dump_redacted_logs "fileconv-server never became ready"
  echo "unhealthy: fileconv-server" >&2
  exit 1
fi

if ! workers_alive; then
  exit 1
fi

"$ROOT/deploy/scripts/seed-dev-all.sh" --skip-init
echo "healthy: fileconv-server + convert/index/embedding workers (web/dist from $MARKHAND_WEB_DIST_DIR)"

set +e
MARKHAND_E2E_REAL=1 \
  MARKHAND_E2E_REAL_BASE_URL="http://${bind_addr}" \
  pnpm --dir "$WEB_DIR" exec playwright test --project=real
playwright_status=$?
set -e

if [[ "$playwright_status" -ne 0 ]]; then
  dump_redacted_logs "Playwright real project failed (exit ${playwright_status})"
  exit "$playwright_status"
fi

exit 0
