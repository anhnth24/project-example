#!/usr/bin/env bash
set -euo pipefail

# Connection variables
export PGHOST="${PGHOST:-127.0.0.1}"
export PGPORT="${PGPORT:-5432}"
export PGUSER="${PGUSER:-markhand}"
export PGPASSWORD="${PGPASSWORD:-markhand_dev_only}"
export PGDATABASE="${PGDATABASE:-markhand}"

# Check for psql
if ! command -v psql &> /dev/null; then
    echo "psql could not be found, attempting to install..."
    sudo apt-get update
    sudo apt-get install -y postgresql-client
fi

# Wait for postgres to be ready
echo "Checking postgres connectivity at ${PGHOST}:${PGPORT}..."
ready=false
for _ in $(seq 1 30); do
    if PGDATABASE=postgres psql -tAc "SELECT 1" &> /dev/null; then
        ready=true
        break
    fi
    sleep 1
done

if [[ "$ready" != "true" ]]; then
    echo "Error: Cannot connect to postgres at ${PGHOST}:${PGPORT}." >&2
    exit 1
fi

SMOKE_DB="markhand_smoke_test_$(date +%s)"
echo "Creating temporary database ${SMOKE_DB}..."
PGDATABASE=postgres psql -v ON_ERROR_STOP=1 -c "CREATE DATABASE \"${SMOKE_DB}\";"

cleanup() {
    echo "Cleaning up temporary database ${SMOKE_DB}..."
    PGDATABASE=postgres psql -v ON_ERROR_STOP=0 -c "
        SELECT pg_terminate_backend(pid) FROM pg_stat_activity
        WHERE datname = '${SMOKE_DB}' AND pid <> pg_backend_pid();
        DROP DATABASE IF EXISTS \"${SMOKE_DB}\";
    " > /dev/null 2>&1 || true
}
trap cleanup EXIT

echo "Applying migrations in order..."
start_time=$SECONDS

# Extract migration files in the order they are listed in crates/server/src/database.rs
MIGRATIONS=$(grep -oE '[0-9]{4}_[a-zA-Z0-9_]+\.sql' crates/server/src/database.rs | awk '!seen[$0]++')

for migration in $MIGRATIONS; do
    echo "  -> Applying ${migration}..."
    PGDATABASE="${SMOKE_DB}" psql -1 -v ON_ERROR_STOP=1 -f "crates/server/migrations/${migration}" > /dev/null
done

duration=$((SECONDS - start_time))
echo "All migrations applied in ${duration}s."

if [ "$duration" -gt 120 ]; then
    echo "Error: Migrations took ${duration}s, exceeding the 2-minute (120s) limit!" >&2
    exit 1
fi

# Check relforcerowsecurity on all tables that have FORCE ROW LEVEL SECURITY
echo "Verifying FORCE ROW LEVEL SECURITY for tables..."
EXPECTED_RLS_TABLES=$(grep -hoE "ALTER TABLE [a-z_]+ FORCE ROW LEVEL SECURITY" crates/server/migrations/*.sql | awk '{print $3}' | sort | uniq)

failed_tables=()
for table in $EXPECTED_RLS_TABLES; do
    has_rls=$(PGDATABASE="${SMOKE_DB}" psql -tAc "SELECT relforcerowsecurity FROM pg_class WHERE relname = '${table}';" || true)
    if [[ "$has_rls" != "t" ]]; then
        echo "  [FAIL] Table '${table}' relforcerowsecurity is '${has_rls}' (expected 't')"
        failed_tables+=("${table}")
    else
        echo "  [OK] Table '${table}' has relforcerowsecurity=true"
    fi
done

if [ ${#failed_tables[@]} -ne 0 ]; then
    echo "Error: The following tables do not have relforcerowsecurity enabled: ${failed_tables[*]}" >&2
    exit 1
fi

echo "Smoke test passed successfully in ${duration}s!"
