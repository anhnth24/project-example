#!/usr/bin/env bash
# Seed synthetic E2E accounts/org-B against the POC compose stack (P1B-O04).
# Requires tagged test stack (enforced by poc-e2e-o04.sh confirm gates).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=poc-compose.sh
source "$ROOT/deploy/scripts/poc-compose.sh"
poc_compose_init

SQL="$ROOT/crates/server/tests/e2e/sql/seed_e2e_accounts.sql"
PASSWORD="${MARKHAND_E2E_PASSWORD:-${MARKHAND_DEV_PASSWORD:-markhand-e2e}}"

if [[ ! -f "$SQL" ]]; then
  echo "missing $SQL" >&2
  exit 1
fi

echo "seeding E2E accounts..."
"${COMPOSE[@]}" exec -T postgres psql \
  -U "${MARKHAND_POSTGRES_USER:-markhand}" \
  -d "${MARKHAND_POSTGRES_DB:-markhand}" \
  --set ON_ERROR_STOP=1 \
  <"$SQL"

hash="$(
  cargo run -q -p fileconv-server --bin dev-hash-password -- "$PASSWORD" \
    2>/dev/null \
    || cargo run -p fileconv-server --bin dev-hash-password -- "$PASSWORD"
)"
hash_sql="${hash//\'/\'\'}"

EMAILS=(
  "admin@poc.example"
  "editor-e2e@poc.example"
  "viewer-e2e@poc.example"
  "owner@org-b.example"
)

updated=0
for email in "${EMAILS[@]}"; do
  if "${COMPOSE[@]}" exec -T postgres psql \
    -U "${MARKHAND_POSTGRES_USER:-markhand}" \
    -d "${MARKHAND_POSTGRES_DB:-markhand}" \
    --set ON_ERROR_STOP=1 \
    -tAc \
    "UPDATE users SET password_hash = '${hash_sql}', updated_at = now() WHERE email = '${email}' RETURNING email;" \
    | grep -q .; then
    echo "set E2E password for $email"
    updated=$((updated + 1))
  else
    echo "warning: no user row for $email" >&2
  fi
done

if [[ "$updated" == "0" ]]; then
  echo "no passwords updated — ensure migrations applied and API started once" >&2
  exit 1
fi

echo "E2E seed complete (password via MARKHAND_E2E_PASSWORD; not printed)"