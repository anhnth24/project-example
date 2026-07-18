#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
ENV_FILE="$ROOT/deploy/dev/.env"

created_env=false
if [[ ! -f "$ENV_FILE" ]]; then
  cp "$ROOT/deploy/dev/.env.example" "$ENV_FILE"
  created_env=true
fi

set -a
# shellcheck disable=SC1090
source "$ENV_FILE"
set +a

"$ROOT/deploy/scripts/bootstrap-server-role.sh"

log_file="$(mktemp)"
cargo run -p fileconv-server >"$log_file" 2>&1 &
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
    "$ROOT/deploy/scripts/seed-poc-org.sh"
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
