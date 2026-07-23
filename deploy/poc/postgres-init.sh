#!/bin/bash
# Parameterized POC bootstrap: migrator owns schema; app is DML-only (O01 / Sol #5).
# Runs via /docker-entrypoint-initdb.d (fresh volume only).
set -euo pipefail

MIG_USER="${MARKHAND_MIGRATOR_DB_USER:-markhand_migrator}"
MIG_PASSWORD="${MARKHAND_MIGRATOR_DB_PASSWORD:-${MARKHAND_APP_DB_PASSWORD:?MARKHAND_APP_DB_PASSWORD or MARKHAND_MIGRATOR_DB_PASSWORD required}}"
APP_USER="${MARKHAND_APP_DB_USER:-markhand_app}"
APP_PASSWORD="${MARKHAND_APP_DB_PASSWORD:?MARKHAND_APP_DB_PASSWORD required for postgres-init}"
DB_NAME="${POSTGRES_DB:-markhand}"

# Identifiers only — reject anything outside [A-Za-z0-9_].
case "$MIG_USER" in
  "" | *[!a-zA-Z0-9_]*)
    echo "invalid MARKHAND_MIGRATOR_DB_USER: ${MIG_USER}" >&2
    exit 1
    ;;
esac
case "$APP_USER" in
  "" | *[!a-zA-Z0-9_]*)
    echo "invalid MARKHAND_APP_DB_USER: ${APP_USER}" >&2
    exit 1
    ;;
esac
case "$DB_NAME" in
  "" | *[!a-zA-Z0-9_]*)
    echo "invalid POSTGRES_DB: ${DB_NAME}" >&2
    exit 1
    ;;
esac

sql_escape() {
  printf "%s" "$1" | sed "s/'/''/g"
}
MIG_PASS_ESC="$(sql_escape "$MIG_PASSWORD")"
APP_PASS_ESC="$(sql_escape "$APP_PASSWORD")"

psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname "$DB_NAME" <<EOSQL
CREATE EXTENSION IF NOT EXISTS pgcrypto;

DO \$\$
BEGIN
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '${MIG_USER}') THEN
    -- INHERIT so GRANT markhand_app → migrator can reassign legacy app-owned objects.
    CREATE ROLE ${MIG_USER} LOGIN PASSWORD '${MIG_PASS_ESC}'
      NOSUPERUSER NOCREATEDB NOCREATEROLE INHERIT;
  END IF;
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '${APP_USER}') THEN
    CREATE ROLE ${APP_USER} LOGIN PASSWORD '${APP_PASS_ESC}'
      NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;
  END IF;
END
\$\$;

GRANT CONNECT ON DATABASE ${DB_NAME} TO ${MIG_USER};
GRANT CONNECT ON DATABASE ${DB_NAME} TO ${APP_USER};
GRANT USAGE, CREATE ON SCHEMA public TO ${MIG_USER};
GRANT USAGE ON SCHEMA public TO ${APP_USER};
REVOKE CREATE ON SCHEMA public FROM ${APP_USER};

ALTER DEFAULT PRIVILEGES FOR ROLE ${MIG_USER} IN SCHEMA public
  GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO ${APP_USER};
ALTER DEFAULT PRIVILEGES FOR ROLE ${MIG_USER} IN SCHEMA public
  GRANT USAGE, SELECT ON SEQUENCES TO ${APP_USER};
ALTER DEFAULT PRIVILEGES FOR ROLE ${MIG_USER} IN SCHEMA public
  GRANT EXECUTE ON FUNCTIONS TO ${APP_USER};
EOSQL
