#!/usr/bin/env bash
# Bring up fileconv-server against the local Compose stack and wait for /health/ready.
# Applies migrations + Qdrant collection (required after migrator/app role split).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
ENV_FILE="$ROOT/deploy/dev/.env"

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

for _ in $(seq 1 60); do
  if curl --fail --silent --show-error \
    "http://${MARKHAND_BIND_ADDR:-127.0.0.1:8787}/api/v1/health/ready" >/dev/null; then
    "$ROOT/deploy/scripts/seed-dev-all.sh" --skip-init
    echo "healthy: fileconv-server"
    exit 0
  fi
  if ! kill -0 "$server_pid" 2>/dev/null; then
    cat "$log_file" >&2
    exit 1
  fi
  sleep 1
done

cat "$log_file" >&2
echo "unhealthy: fileconv-server" >&2
exit 1
