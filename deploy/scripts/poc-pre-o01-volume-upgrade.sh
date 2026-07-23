#!/usr/bin/env bash
# Real Compose pre-O01 volume upgrade: existing postgres volume without migrator
# role → privileged idempotent bootstrap → migrate → assert audit ownership/grants.
#
# Uses a dedicated Compose project + host ports so it never tears down markhand-poc.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=poc-compose.sh
source "$ROOT/deploy/scripts/poc-compose.sh"

PROJECT="${MARKHAND_O01_UPGRADE_PROJECT:-markhand-o01-upgrade}"
PG_PORT="${MARKHAND_O01_UPGRADE_PG_PORT:-54331}"

# Base env from deploy/.env, then force isolated project/ports via a temp env file
# (compose top-level `name:` reads MARKHAND_COMPOSE_PROJECT from --env-file).
poc_compose_init
TMP_ENV="$(mktemp "${TMPDIR:-/tmp}/markhand-o01-upgrade.env.XXXXXX")"
write_tmp_env() {
  {
    cat "$ENV_FILE"
    echo
    echo "MARKHAND_COMPOSE_PROJECT=${PROJECT}"
    echo "MARKHAND_POSTGRES_PORT=${PG_PORT}"
    echo "MARKHAND_API_PORT=${MARKHAND_O01_UPGRADE_API_PORT:-8789}"
    echo "MARKHAND_QDRANT_HTTP_PORT=${MARKHAND_O01_UPGRADE_QDRANT_HTTP_PORT:-6353}"
    echo "MARKHAND_QDRANT_GRPC_PORT=${MARKHAND_O01_UPGRADE_QDRANT_GRPC_PORT:-6354}"
    echo "MARKHAND_MINIO_API_PORT=${MARKHAND_O01_UPGRADE_MINIO_API_PORT:-9020}"
    echo "MARKHAND_MINIO_CONSOLE_PORT=${MARKHAND_O01_UPGRADE_MINIO_CONSOLE_PORT:-9021}"
    echo "MARKHAND_EMBEDDING_PORT=${MARKHAND_O01_UPGRADE_EMBEDDING_PORT:-8091}"
  } >"$TMP_ENV"
}
write_tmp_env
# Shell env wins over --env-file for compose interpolation — force overrides here.
export MARKHAND_COMPOSE_PROJECT="$PROJECT"
export MARKHAND_POSTGRES_PORT="$PG_PORT"
export MARKHAND_API_PORT="${MARKHAND_O01_UPGRADE_API_PORT:-8789}"
export MARKHAND_QDRANT_HTTP_PORT="${MARKHAND_O01_UPGRADE_QDRANT_HTTP_PORT:-6353}"
export MARKHAND_QDRANT_GRPC_PORT="${MARKHAND_O01_UPGRADE_QDRANT_GRPC_PORT:-6354}"
export MARKHAND_MINIO_API_PORT="${MARKHAND_O01_UPGRADE_MINIO_API_PORT:-9020}"
export MARKHAND_MINIO_CONSOLE_PORT="${MARKHAND_O01_UPGRADE_MINIO_CONSOLE_PORT:-9021}"
export MARKHAND_EMBEDDING_PORT="${MARKHAND_O01_UPGRADE_EMBEDDING_PORT:-8091}"
export COMPOSE_PROJECT_NAME="$PROJECT"

# Never reuse the pre-rendered nolimit file from poc_compose_init — it baked the
# live POC project/ports. Re-render under the upgrade overrides when needed.
COMPOSE_FILE_EFFECTIVE="$COMPOSE_FILE"
if poc_cgroup_limits_broken || [[ -n "${POC_FORCE_NOLIMIT_COMPOSE:-}" ]]; then
  ENV_FILE="$TMP_ENV"
  COMPOSE_FILE_EFFECTIVE="$(poc_write_nolimit_compose)"
fi
COMPOSE_UP=(docker compose --env-file "$TMP_ENV" -p "$PROJECT" -f "$COMPOSE_FILE_EFFECTIVE")

PASS=0
FAIL=0
pass() { echo "PASS: $*"; PASS=$((PASS + 1)); }
fail() { echo "FAIL: $*" >&2; FAIL=$((FAIL + 1)); }

cleanup_stack() {
  write_tmp_env
  "${COMPOSE_UP[@]}" down -v --remove-orphans >/dev/null 2>&1 || true
}
cleanup_all() {
  cleanup_stack
  rm -f "$TMP_ENV"
}
trap cleanup_all EXIT

echo "== P1B-O01 pre-O01 Compose volume upgrade =="
echo "project=$PROJECT postgres_port=$PG_PORT"

cleanup_stack
write_tmp_env
"${COMPOSE_UP[@]}" up -d postgres
echo "waiting for postgres health..."
for _ in $(seq 1 60); do
  if "${COMPOSE_UP[@]}" exec -T postgres pg_isready -U "${MARKHAND_POSTGRES_USER:-markhand}" \
    -d "${MARKHAND_POSTGRES_DB:-markhand}" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
"${COMPOSE_UP[@]}" exec -T postgres pg_isready -U "${MARKHAND_POSTGRES_USER:-markhand}" \
  -d "${MARKHAND_POSTGRES_DB:-markhand}" >/dev/null
pass "postgres healthy on fresh volume"

DB="${MARKHAND_POSTGRES_DB:-markhand}"
SUPER="${MARKHAND_POSTGRES_USER:-markhand}"
APP_USER="${MARKHAND_APP_DB_USER:-markhand_app}"
APP_PASS="${MARKHAND_APP_DB_PASSWORD:-markhand_app_poc_change_me}"
MIG_USER="${MARKHAND_MIGRATOR_DB_USER:-markhand_migrator}"

psql_c() {
  "${COMPOSE_UP[@]}" exec -T -e PGPASSWORD="${MARKHAND_POSTGRES_PASSWORD:-markhand_poc_change_me}" \
    postgres psql -v ON_ERROR_STOP=1 -U "$SUPER" -d "$DB" "$@"
}

migrate_run() {
  local mig_url="$1"
  local app_url="$2"
  "${COMPOSE_UP[@]}" run --rm --no-deps \
    -e "MARKHAND_MIGRATOR_DATABASE_URL=${mig_url}" \
    -e "MARKHAND_DATABASE_URL=${app_url}" \
    -e "MARKHAND_PROFILE=${MARKHAND_PROFILE:-dev}" \
    -e "MARKHAND_BIND_ADDR=127.0.0.1:8787" \
    -e "MARKHAND_QDRANT_URL=http://qdrant:6333" \
    -e "MARKHAND_MINIO_URL=http://minio:9000" \
    -e "MARKHAND_MINIO_ACCESS_KEY=${MARKHAND_MINIO_ACCESS_KEY:-markhand_app}" \
    -e "MARKHAND_MINIO_SECRET_KEY=${MARKHAND_MINIO_SECRET_KEY:-markhand_app_poc_change_me}" \
    -e "MARKHAND_MINIO_BUCKET=${MARKHAND_MINIO_BUCKET:-markhand-documents}" \
    -e "MARKHAND_AUTH_ISSUER=${MARKHAND_AUTH_ISSUER:-http://127.0.0.1:8788}" \
    -e "MARKHAND_AUTH_AUDIENCE=${MARKHAND_AUTH_AUDIENCE:-markhand-poc}" \
    -e "MARKHAND_AUTH_SIGNING_KEY=${MARKHAND_AUTH_SIGNING_KEY:-poc-only-signing-key-at-least-32-bytes}" \
    -e "MARKHAND_AUTH_KID=${MARKHAND_AUTH_KID:-poc-key-1}" \
    -e "MARKHAND_INDEX_SIGNATURE=${MARKHAND_INDEX_SIGNATURE:-72dda20007ffb7fbe293612091103321eb9e4e0e4a0517a5f3413e31a2978874}" \
    migrate
}

# Seed full schema as superuser (bootstrap), then force app ownership / drop migrator.
SUPER_URL="postgres://${SUPER}:${MARKHAND_POSTGRES_PASSWORD:-markhand_poc_change_me}@postgres:5432/${DB}"
APP_URL="postgres://${APP_USER}:${APP_PASS}@postgres:5432/${DB}"
migrate_run "$SUPER_URL" "$APP_URL"
pass "seed migrations applied (bootstrap/superuser path)"

psql_c <<SQL
DO \$\$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '${APP_USER}') THEN
    ALTER TABLE IF EXISTS audit_log OWNER TO ${APP_USER};
    BEGIN
      ALTER FUNCTION audit_log_enforce_immutability() OWNER TO ${APP_USER};
    EXCEPTION WHEN undefined_function THEN NULL;
    END;
    BEGIN
      ALTER FUNCTION audit_log_validate_insert() OWNER TO ${APP_USER};
    EXCEPTION WHEN undefined_function THEN NULL;
    END;
  END IF;
END
\$\$;
REASSIGN OWNED BY ${MIG_USER} TO ${SUPER};
DROP OWNED BY ${MIG_USER};
DROP ROLE IF EXISTS ${MIG_USER};
DELETE FROM markhand_schema_migrations
 WHERE name = '0028_expand_audit_ownership_migrator.sql';
SQL
OWNER="$(psql_c -Atc "SELECT pg_get_userbyid(c.relowner) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relname='audit_log'")"
[[ "$OWNER" == "$APP_USER" ]] && pass "pre-O01 audit_log owned by app ($OWNER)" || fail "pre-O01 owner=$OWNER want=$APP_USER"

"${COMPOSE_UP[@]}" run --rm --no-deps db-bootstrap
pass "db-bootstrap completed (idempotent migrator/app roles)"

MIG_URL="postgres://${MIG_USER}:${MARKHAND_MIGRATOR_DB_PASSWORD:-markhand_migrator_poc_change_me}@postgres:5432/${DB}"
migrate_run "$MIG_URL" "$APP_URL"
pass "migrate completed via dedicated migrator after bootstrap"

OWNER_AFTER="$(psql_c -Atc "SELECT pg_get_userbyid(c.relowner) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relname='audit_log'")"
[[ "$OWNER_AFTER" != "$APP_USER" ]] && pass "audit_log no longer app-owned ($OWNER_AFTER)" || fail "audit_log still app-owned"
[[ "$OWNER_AFTER" == "$MIG_USER" ]] && pass "audit_log owned by migrator" || fail "audit_log owner=$OWNER_AFTER want=$MIG_USER"

for fn in audit_log_enforce_immutability audit_log_validate_insert; do
  fn_owner="$(psql_c -Atc "SELECT pg_get_userbyid(p.proowner) FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname='public' AND p.proname='${fn}' LIMIT 1")"
  [[ "$fn_owner" != "$APP_USER" ]] && pass "$fn not app-owned ($fn_owner)" || fail "$fn still app-owned"
done

BAD_GRANTS="$(psql_c -Atc "SELECT COUNT(*) FROM information_schema.role_table_grants WHERE grantee='${APP_USER}' AND table_name='audit_log' AND privilege_type IN ('UPDATE','DELETE','TRUNCATE','TRIGGER','REFERENCES')")"
[[ "$BAD_GRANTS" == "0" ]] && pass "app exact grants SELECT+INSERT only" || fail "app has forbidden audit_log grants count=$BAD_GRANTS"

echo "summary: pass=$PASS fail=$FAIL"
[[ "$FAIL" -eq 0 ]]
