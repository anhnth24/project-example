#!/usr/bin/env bash
# Bootstrap migrator + app + worker DB roles for local/dev (O01 / Sol #5 / 1C-08).
# Migrator owns schema changes; markhand_app is DML-only (no CREATE / no audit mutate).
# markhand_worker is LOGIN least-privilege (NOBYPASSRLS); table grants come from
# migration 0035 when the role exists before migrate. Passwords come from env only.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
COMPOSE=(docker compose -f "$ROOT/deploy/dev/compose.yml")

MIG_USER="${MARKHAND_MIGRATOR_DB_USER:-markhand_migrator}"
MIG_PASSWORD="${MARKHAND_MIGRATOR_DB_PASSWORD:-markhand_migrator_dev_only}"
APP_USER="${MARKHAND_APP_DB_USER:-markhand_app}"
APP_PASSWORD="${MARKHAND_APP_DB_PASSWORD:-markhand_app_dev_only}"
WORKER_USER="${MARKHAND_WORKER_DB_USER:-markhand_worker}"
WORKER_PASSWORD="${MARKHAND_WORKER_DB_PASSWORD:-markhand_worker_dev_only}"
DB_NAME="${MARKHAND_POSTGRES_DB:-markhand}"
PG_USER="${MARKHAND_POSTGRES_USER:-markhand}"

sql_escape() {
  printf "%s" "$1" | sed "s/'/''/g"
}
MIG_PASS_ESC="$(sql_escape "$MIG_PASSWORD")"
APP_PASS_ESC="$(sql_escape "$APP_PASSWORD")"
WORKER_PASS_ESC="$(sql_escape "$WORKER_PASSWORD")"

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
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '${WORKER_USER}') THEN
    CREATE ROLE ${WORKER_USER} LOGIN PASSWORD '${WORKER_PASS_ESC}'
      NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOBYPASSRLS;
  ELSE
    ALTER ROLE ${WORKER_USER} WITH LOGIN PASSWORD '${WORKER_PASS_ESC}'
      NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOBYPASSRLS;
  END IF;
END
\$\$;

GRANT CONNECT ON DATABASE ${DB_NAME} TO ${MIG_USER};
GRANT CONNECT ON DATABASE ${DB_NAME} TO ${APP_USER};
GRANT CONNECT ON DATABASE ${DB_NAME} TO ${WORKER_USER};

-- Migrator may create/own schema objects; app/worker may not.
GRANT USAGE, CREATE ON SCHEMA public TO ${MIG_USER};
GRANT USAGE ON SCHEMA public TO ${APP_USER};
GRANT USAGE ON SCHEMA public TO ${WORKER_USER};
REVOKE CREATE ON SCHEMA public FROM ${APP_USER};
REVOKE CREATE ON SCHEMA public FROM ${WORKER_USER};

-- PG16+: membership inherit so migrator can reassign legacy app-owned objects.
GRANT ${APP_USER} TO ${MIG_USER} WITH INHERIT TRUE;

-- Dev seed scripts (deploy/scripts/seed*.sh) create tables as ${PG_USER} before
-- migrations run; 0027's GRANT ... ON ALL TABLES IN SCHEMA public then fails with
-- 42501 unless the migrator can grant on them, so hand ownership to the migrator.
DO \$\$
DECLARE
  t record;
BEGIN
  FOR t IN
    SELECT schemaname, tablename FROM pg_tables
    WHERE schemaname = 'public' AND tableowner = '${PG_USER}'
  LOOP
    EXECUTE format('ALTER TABLE %I.%I OWNER TO ${MIG_USER}', t.schemaname, t.tablename);
  END LOOP;
END
\$\$;

-- Default privileges for objects created by migrator.
ALTER DEFAULT PRIVILEGES FOR ROLE ${MIG_USER} IN SCHEMA public
  GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO ${APP_USER};
ALTER DEFAULT PRIVILEGES FOR ROLE ${MIG_USER} IN SCHEMA public
  GRANT USAGE, SELECT ON SEQUENCES TO ${APP_USER};
ALTER DEFAULT PRIVILEGES FOR ROLE ${MIG_USER} IN SCHEMA public
  GRANT EXECUTE ON FUNCTIONS TO ${APP_USER};
SQL

echo "bootstrapped migrator (${MIG_USER}) + app (${APP_USER}) + worker (${WORKER_USER}) roles"
