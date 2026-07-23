#!/usr/bin/env bash
# One-shot migrator: applies schema as markhand_migrator (never markhand_app).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
MIG_USER="${MARKHAND_MIGRATOR_DB_USER:-markhand_migrator}"
MIG_PASSWORD="${MARKHAND_MIGRATOR_DB_PASSWORD:-markhand_migrator_dev_only}"
APP_DB="${MARKHAND_POSTGRES_DB:-markhand}"
HOST="${MARKHAND_POSTGRES_HOST:-127.0.0.1}"
PORT="${MARKHAND_POSTGRES_PORT:-5432}"

if [[ -z "${MARKHAND_MIGRATOR_DATABASE_URL:-}" ]]; then
  export MARKHAND_MIGRATOR_DATABASE_URL="postgres://${MIG_USER}:${MIG_PASSWORD}@${HOST}:${PORT}/${APP_DB}"
fi

# Refuse app-role URLs even if mis-set.
case "${MARKHAND_MIGRATOR_DATABASE_URL}" in
  *://markhand_app:*|*://markhand_app@*)
    echo "migrate.sh refuses MARKHAND_MIGRATOR_DATABASE_URL using markhand_app" >&2
    exit 1
    ;;
esac

# Ensure roles exist (dev/bootstrap).
if [[ -x "$ROOT/deploy/scripts/bootstrap-server-role.sh" ]]; then
  "$ROOT/deploy/scripts/bootstrap-server-role.sh" || true
fi

cd "$ROOT"
if [[ -x "$ROOT/target/release/fileconv-server" ]]; then
  BIN="$ROOT/target/release/fileconv-server"
elif [[ -x "$ROOT/target/debug/fileconv-server" ]]; then
  BIN="$ROOT/target/debug/fileconv-server"
else
  cargo build -p fileconv-server
  BIN="$ROOT/target/debug/fileconv-server"
fi

exec "$BIN" --migrate-only
