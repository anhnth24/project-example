#!/usr/bin/env bash
# Privileged idempotent role bootstrap for POC (existing volumes + fresh).
# Creates markhand_migrator / markhand_app / markhand_worker, grants migrator
# CREATE + GRANT app TO migrator WITH INHERIT TRUE. Safe to re-run.
# Worker passwords come from env only — never echoed or logged as a full URL.
# Table DML grants for markhand_worker remain migration 0035's job when the
# role exists before migrate; when tables already exist (re-bootstrap after a
# prior migrate that skipped 0035), apply the same grant set idempotently here.
# Runs as POSTGRES_USER (superuser) — must execute before migrate.
set -euo pipefail

MIG_USER="${MARKHAND_MIGRATOR_DB_USER:-markhand_migrator}"
MIG_PASSWORD="${MARKHAND_MIGRATOR_DB_PASSWORD:?MARKHAND_MIGRATOR_DB_PASSWORD required}"
APP_USER="${MARKHAND_APP_DB_USER:-markhand_app}"
APP_PASSWORD="${MARKHAND_APP_DB_PASSWORD:?MARKHAND_APP_DB_PASSWORD required}"
WORKER_USER="${MARKHAND_WORKER_DB_USER:-markhand_worker}"
WORKER_PASSWORD="${MARKHAND_WORKER_DB_PASSWORD:?MARKHAND_WORKER_DB_PASSWORD required}"
DB_NAME="${POSTGRES_DB:-${MARKHAND_POSTGRES_DB:-markhand}}"
PGUSER="${POSTGRES_USER:-markhand}"

# Identifiers only.
case "$MIG_USER" in "" | *[!a-zA-Z0-9_]*) echo "invalid migrator user" >&2; exit 1 ;; esac
case "$APP_USER" in "" | *[!a-zA-Z0-9_]*) echo "invalid app user" >&2; exit 1 ;; esac
case "$WORKER_USER" in "" | *[!a-zA-Z0-9_]*) echo "invalid worker user" >&2; exit 1 ;; esac
case "$DB_NAME" in "" | *[!a-zA-Z0-9_]*) echo "invalid db name" >&2; exit 1 ;; esac

sql_escape() { printf "%s" "$1" | sed "s/'/''/g"; }
MIG_PASS_ESC="$(sql_escape "$MIG_PASSWORD")"
APP_PASS_ESC="$(sql_escape "$APP_PASSWORD")"
WORKER_PASS_ESC="$(sql_escape "$WORKER_PASSWORD")"

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
GRANT USAGE, CREATE ON SCHEMA public TO ${MIG_USER};
GRANT USAGE ON SCHEMA public TO ${APP_USER};
GRANT USAGE ON SCHEMA public TO ${WORKER_USER};
REVOKE CREATE ON SCHEMA public FROM ${APP_USER};
REVOKE CREATE ON SCHEMA public FROM ${WORKER_USER};

-- PG16+: inherited membership so migrator can reassign legacy app-owned objects.
GRANT ${APP_USER} TO ${MIG_USER} WITH INHERIT TRUE;

ALTER DEFAULT PRIVILEGES FOR ROLE ${MIG_USER} IN SCHEMA public
  GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO ${APP_USER};
ALTER DEFAULT PRIVILEGES FOR ROLE ${MIG_USER} IN SCHEMA public
  GRANT USAGE, SELECT ON SEQUENCES TO ${APP_USER};
ALTER DEFAULT PRIVILEGES FOR ROLE ${MIG_USER} IN SCHEMA public
  GRANT EXECUTE ON FUNCTIONS TO ${APP_USER};

-- Idempotent mirror of migration 0035 grants for volumes where 0035 already
-- ran before markhand_worker existed. No-op when tables are not yet created
-- (fresh path relies on 0035 after this bootstrap).
DO \$\$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '${WORKER_USER}')
     AND to_regclass('public.jobs') IS NOT NULL THEN
    EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE
      jobs, outbox_events, event_log,
      documents, document_versions, derived_artifacts, chunks, claims,
      index_metadata, index_generation_backfills, embedding_batches,
      vector_cleanup_intents,
      quota_reservations, usage_counters
      TO ${WORKER_USER}';
    EXECUTE 'GRANT SELECT ON TABLE org_quotas TO ${WORKER_USER}';
    EXECUTE 'GRANT SELECT, INSERT ON TABLE audit_log TO ${WORKER_USER}';
    BEGIN
      EXECUTE 'REVOKE UPDATE, DELETE, TRUNCATE ON TABLE audit_log FROM ${WORKER_USER}';
    EXCEPTION WHEN insufficient_privilege OR undefined_object THEN
      NULL;
    END;
    EXECUTE 'GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO ${WORKER_USER}';
    EXECUTE 'GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA public TO ${WORKER_USER}';
  END IF;
END
\$\$;
EOSQL

echo "poc bootstrap-roles: migrator=${MIG_USER} app=${APP_USER} worker=${WORKER_USER} db=${DB_NAME} ok"
