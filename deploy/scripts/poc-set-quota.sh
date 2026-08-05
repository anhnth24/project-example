#!/usr/bin/env bash
# Apply an explicit post-migration storage quota to the seeded POC organization.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=poc-compose.sh
source "$ROOT/deploy/scripts/poc-compose.sh"
poc_compose_init

quota_bytes="${MARKHAND_POC_MAX_STORAGE_BYTES:-}"
if [[ -z "$quota_bytes" ]]; then
  exit 0
fi
if [[ ! "$quota_bytes" =~ ^[0-9]+$ ]]; then
  echo "MARKHAND_POC_MAX_STORAGE_BYTES must be an unsigned integer" >&2
  exit 1
fi

org_id="${MARKHAND_WORKER_ORG_ID:-11111111-1111-1111-1111-111111111111}"
if [[ ! "$org_id" =~ ^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$ ]]; then
  echo "MARKHAND_WORKER_ORG_ID must be a UUID" >&2
  exit 1
fi

psql_args=(
  exec -T postgres psql
  -U "${MARKHAND_POSTGRES_USER:-markhand}"
  -d "${MARKHAND_POSTGRES_DB:-markhand}"
  --set ON_ERROR_STOP=1
)

row_count="$(
  "${COMPOSE[@]}" "${psql_args[@]}" -tAc \
    "SELECT count(*) FROM org_quotas WHERE org_id = '${org_id}'::uuid;"
)"
row_count="${row_count//[[:space:]]/}"
if [[ "$row_count" != "1" ]]; then
  echo "seeded organization quota row not found" >&2
  exit 1
fi

"${COMPOSE[@]}" "${psql_args[@]}" \
  --set "quota_bytes=$quota_bytes" \
  --set "org_id=$org_id" \
  -c "UPDATE org_quotas
      SET max_storage_bytes = :'quota_bytes'::bigint, updated_at = now()
      WHERE org_id = :'org_id'::uuid;" \
  >/dev/null

echo "updated seeded organization storage quota to ${quota_bytes} bytes"
