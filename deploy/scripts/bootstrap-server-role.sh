#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
COMPOSE=(docker compose -f "$ROOT/deploy/dev/compose.yml")

"${COMPOSE[@]}" exec -T postgres psql \
  -U "${MARKHAND_POSTGRES_USER:-markhand}" \
  -d "${MARKHAND_POSTGRES_DB:-markhand}" \
  --set ON_ERROR_STOP=1 <<'SQL'
CREATE EXTENSION IF NOT EXISTS pgcrypto;
DO $$
BEGIN
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'markhand_app') THEN
    CREATE ROLE markhand_app LOGIN PASSWORD 'markhand_app_dev_only'
      NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;
  END IF;
END
$$;
GRANT CONNECT ON DATABASE markhand TO markhand_app;
GRANT USAGE, CREATE ON SCHEMA public TO markhand_app;
SQL

echo "bootstrapped local non-superuser server role"
