#!/usr/bin/env bash
# Privileged idempotent role bootstrap for POC (existing volumes + fresh).
# Creates markhand_migrator / markhand_app, grants migrator CREATE +
# GRANT app TO migrator WITH INHERIT TRUE. Safe to re-run.
# Runs as POSTGRES_USER (superuser) — must execute before migrate.
set -euo pipefail

MIG_USER="${MARKHAND_MIGRATOR_DB_USER:-markhand_migrator}"
MIG_PASSWORD="${MARKHAND_MIGRATOR_DB_PASSWORD:?MARKHAND_MIGRATOR_DB_PASSWORD required}"
APP_USER="${MARKHAND_APP_DB_USER:-markhand_app}"
APP_PASSWORD="${MARKHAND_APP_DB_PASSWORD:?MARKHAND_APP_DB_PASSWORD required}"
DB_NAME="${POSTGRES_DB:-${MARKHAND_POSTGRES_DB:-markhand}}"
PGUSER="${POSTGRES_USER:-markhand}"

# Identifiers only.
case "$MIG_USER" in "" | *[!a-zA-Z0-9_]*) echo "invalid migrator user" >&2; exit 1 ;; esac
case "$APP_USER" in "" | *[!a-zA-Z0-9_]*) echo "invalid app user" >&2; exit 1 ;; esac
case "$DB_NAME" in "" | *[!a-zA-Z0-9_]*) echo "invalid db name" >&2; exit 1 ;; esac

sql_escape() { printf "%s" "$1" | sed "s/'/''/g"; }
MIG_PASS_ESC="$(sql_escape "$MIG_PASSWORD")"
APP_PASS_ESC="$(sql_escape "$APP_PASSWORD")"

PGHOST="${PGHOST:-postgres}"
PGPORT="${PGPORT:-5432}"
export PGHOST PGPORT
psql -v ON_ERROR_STOP=1 --username "$PGUSER" --dbname "$DB_NAME" <<EOSQL
CREATE EXTENSION IF NOT EXISTS pgcrypto;

DO \$\$
BEGIN
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '${MIG_USER}') THEN
    CREATE ROLE ${MIG_USER} LOGIN PASSWORD '${MIG_PASS_ESC}'
      NOSUPERUSER NOCREATEDB NOCREATEROLE INHERIT;
  ELSE
    ALTER ROLE ${MIG_USER} WITH INHERIT LOGIN PASSWORD '${MIG_PASS_ESC}';
  END IF;
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '${APP_USER}') THEN
    CREATE ROLE ${APP_USER} LOGIN PASSWORD '${APP_PASS_ESC}'
      NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;
  ELSE
    ALTER ROLE ${APP_USER} WITH LOGIN PASSWORD '${APP_PASS_ESC}';
  END IF;
END
\$\$;

GRANT CONNECT ON DATABASE ${DB_NAME} TO ${MIG_USER};
GRANT CONNECT ON DATABASE ${DB_NAME} TO ${APP_USER};
GRANT USAGE, CREATE ON SCHEMA public TO ${MIG_USER};
GRANT USAGE ON SCHEMA public TO ${APP_USER};
REVOKE CREATE ON SCHEMA public FROM ${APP_USER};

-- PG16+: inherited membership so migrator can reassign legacy app-owned objects.
GRANT ${APP_USER} TO ${MIG_USER} WITH INHERIT TRUE;

ALTER DEFAULT PRIVILEGES FOR ROLE ${MIG_USER} IN SCHEMA public
  GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO ${APP_USER};
ALTER DEFAULT PRIVILEGES FOR ROLE ${MIG_USER} IN SCHEMA public
  GRANT USAGE, SELECT ON SEQUENCES TO ${APP_USER};
ALTER DEFAULT PRIVILEGES FOR ROLE ${MIG_USER} IN SCHEMA public
  GRANT EXECUTE ON FUNCTIONS TO ${APP_USER};
EOSQL

echo "poc bootstrap-roles: migrator=${MIG_USER} app=${APP_USER} db=${DB_NAME} ok"
