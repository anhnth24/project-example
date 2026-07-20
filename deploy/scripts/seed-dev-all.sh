#!/usr/bin/env bash
# Seed all local dev data after fileconv-server has applied migrations at least once.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
COMPOSE=(docker compose -f "$ROOT/deploy/dev/compose.yml")
SKIP_INIT=false

usage() {
  cat <<EOF
Usage: deploy/scripts/seed-dev-all.sh [--skip-init]

Runs init-dev-env (unless --skip-init), bootstrap-server-role, stack metadata seed,
POC org membership, dev passwords, and prints defaults.

Requires: Docker stack up, markhand_schema_migrations includes 0011_expand_poc_seed.sql
(start fileconv-server once if this fails).

Environment:
  MARKHAND_DEV_PASSWORD   login password (default: markhand-dev)
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-init) SKIP_INIT=true; shift ;;
    -h | --help) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ "$SKIP_INIT" == false ]]; then
  "$ROOT/deploy/scripts/init-dev-env.sh"
fi

ENV_FILE="$ROOT/deploy/dev/.env"
if [[ ! -f "$ENV_FILE" ]]; then
  echo "missing $ENV_FILE — run init-dev-env.sh first" >&2
  exit 1
fi

set -a
# shellcheck disable=SC1090
source "$ENV_FILE"
set +a

postgres_id="$("${COMPOSE[@]}" ps -q postgres 2>/dev/null || true)"
if [[ -z "$postgres_id" ]]; then
  echo "postgres container not running — run: make dev-up" >&2
  exit 1
fi

migration_ok="$("${COMPOSE[@]}" exec -T postgres psql \
  -U "${MARKHAND_POSTGRES_USER:-markhand}" \
  -d "${MARKHAND_POSTGRES_DB:-markhand}" \
  -tAc "SELECT count(*) FROM markhand_schema_migrations WHERE name = '0011_expand_poc_seed.sql'" \
  2>/dev/null || echo "0")"
migration_ok="${migration_ok//[[:space:]]/}"
if [[ "$migration_ok" != "1" ]]; then
  cat >&2 <<EOF
migrations not ready (0011_expand_poc_seed.sql missing).

Start the API server once so it applies migrations, then re-run this script:
  set -a && source deploy/dev/.env && set +a
  deploy/scripts/bootstrap-server-role.sh
  cargo run -p fileconv-server
  # wait for listening, Ctrl+C, then:
  deploy/scripts/seed-dev-all.sh --skip-init
EOF
  exit 1
fi

"$ROOT/deploy/scripts/bootstrap-server-role.sh"
"$ROOT/deploy/scripts/seed.sh"
"$ROOT/deploy/scripts/seed-poc-org.sh"
"$ROOT/deploy/scripts/seed-dev-password.sh"
"$ROOT/deploy/scripts/seed-dev-metadata.sh"

echo ""
"$ROOT/deploy/scripts/print-dev-defaults.sh"
