#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
COMPOSE=(docker compose -f "$ROOT/deploy/dev/compose.yml")

PASSWORD="${MARKHAND_DEV_PASSWORD:-markhand-dev}"
EMAILS=(
  "admin@poc.example"
  "owner@example.test"
)

if [[ ! -f "$ROOT/deploy/dev/.env" ]]; then
  echo "missing deploy/dev/.env — copy deploy/dev/.env.example first" >&2
  exit 1
fi

set -a
# shellcheck disable=SC1090
source "$ROOT/deploy/dev/.env"
set +a

hash="$(
  cargo run -q -p fileconv-server --bin dev-hash-password -- "$PASSWORD" \
    2>/dev/null \
    || cargo run -p fileconv-server --bin dev-hash-password -- "$PASSWORD"
)"

hash_sql="${hash//\'/\'\'}"
updated=0
for email in "${EMAILS[@]}"; do
  if "${COMPOSE[@]}" exec -T postgres psql \
    -U "${MARKHAND_POSTGRES_USER:-markhand}" \
    -d "${MARKHAND_POSTGRES_DB:-markhand}" \
    --set ON_ERROR_STOP=1 \
    -tAc \
    "UPDATE users SET password_hash = '${hash_sql}', updated_at = now() WHERE email = '${email}' RETURNING email;" \
    | grep -q .; then
    echo "set dev password for $email"
    updated=$((updated + 1))
  else
    echo "warning: no user row for $email (run seed-dev-all.sh after migrations)" >&2
  fi
done

if [[ "$updated" == "0" ]]; then
  echo "no passwords updated — start fileconv-server once, then re-run seed-dev-all.sh" >&2
  exit 1
fi

echo "dev login password: $PASSWORD"
