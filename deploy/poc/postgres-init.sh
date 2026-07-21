#!/bin/bash
# Parameterized POC bootstrap for the non-superuser application role.
# Runs via /docker-entrypoint-initdb.d (fresh volume only).
set -euo pipefail

APP_USER="${MARKHAND_APP_DB_USER:-markhand_app}"
APP_PASSWORD="${MARKHAND_APP_DB_PASSWORD:?MARKHAND_APP_DB_PASSWORD required for postgres-init}"
DB_NAME="${POSTGRES_DB:-markhand}"

# Identifiers only — reject anything outside [A-Za-z0-9_].
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
PASS_ESC="$(sql_escape "$APP_PASSWORD")"

psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname "$DB_NAME" <<EOSQL
CREATE EXTENSION IF NOT EXISTS pgcrypto;

DO \$\$
BEGIN
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '${APP_USER}') THEN
    CREATE ROLE ${APP_USER} LOGIN PASSWORD '${PASS_ESC}'
      NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;
  END IF;
END
\$\$;

GRANT CONNECT ON DATABASE ${DB_NAME} TO ${APP_USER};
GRANT USAGE, CREATE ON SCHEMA public TO ${APP_USER};
EOSQL
