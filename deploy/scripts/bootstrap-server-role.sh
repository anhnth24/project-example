#!/usr/bin/env bash
# Bootstrap migrator + app DB roles for local/dev (O01 / Sol #5).
# Migrator owns schema changes; markhand_app is DML-only (no CREATE / no audit mutate).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
COMPOSE=(docker compose -f "$ROOT/deploy/dev/compose.yml")

MIG_USER="${MARKHAND_MIGRATOR_DB_USER:-markhand_migrator}"
MIG_PASSWORD="${MARKHAND_MIGRATOR_DB_PASSWORD:-markhand_migrator_dev_only}"
APP_USER="${MARKHAND_APP_DB_USER:-markhand_app}"
APP_PASSWORD="${MARKHAND_APP_DB_PASSWORD:-markhand_app_dev_only}"
DB_NAME="${MARKHAND_POSTGRES_DB:-markhand}"
PG_USER="${MARKHAND_POSTGRES_USER:-markhand}"

sql_escape() {
  printf "%s" "$1" | sed "s/'/''/g"
}
MIG_PASS_ESC="$(sql_escape "$MIG_PASSWORD")"
APP_PASS_ESC="$(sql_escape "$APP_PASSWORD")"

"${COMPOSE[@]}" exec -T postgres psql \
  -U "$PG_USER" \
  -d "$DB_NAME" \
  --set ON_ERROR_STOP=1 <<SQL
CREATE EXTENSION IF NOT EXISTS pgcrypto;

DO \$\$
BEGIN
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '${MIG_USER}') THEN
    CREATE ROLE ${MIG_USER} LOGIN PASSWORD '${MIG_PASS_ESC}'
      NOSUPERUSER NOCREATEDB NOCREATEROLE INHERIT;
  ELSE
    ALTER ROLE ${MIG_USER} WITH INHERIT;
  END IF;
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '${APP_USER}') THEN
    CREATE ROLE ${APP_USER} LOGIN PASSWORD '${APP_PASS_ESC}'
      NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;
  END IF;
END
\$\$;

GRANT CONNECT ON DATABASE ${DB_NAME} TO ${MIG_USER};
GRANT CONNECT ON DATABASE ${DB_NAME} TO ${APP_USER};

-- Migrator may create/own schema objects; app may not.
GRANT USAGE, CREATE ON SCHEMA public TO ${MIG_USER};
GRANT USAGE ON SCHEMA public TO ${APP_USER};
REVOKE CREATE ON SCHEMA public FROM ${APP_USER};

-- PG16+: membership inherit so migrator can reassign legacy app-owned objects.
GRANT ${APP_USER} TO ${MIG_USER} WITH INHERIT TRUE;

-- Default privileges for objects created by migrator.
ALTER DEFAULT PRIVILEGES FOR ROLE ${MIG_USER} IN SCHEMA public
  GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO ${APP_USER};
ALTER DEFAULT PRIVILEGES FOR ROLE ${MIG_USER} IN SCHEMA public
  GRANT USAGE, SELECT ON SEQUENCES TO ${APP_USER};
ALTER DEFAULT PRIVILEGES FOR ROLE ${MIG_USER} IN SCHEMA public
  GRANT EXECUTE ON FUNCTIONS TO ${APP_USER};
SQL

echo "bootstrapped migrator (${MIG_USER}) + app (${APP_USER}) roles"
