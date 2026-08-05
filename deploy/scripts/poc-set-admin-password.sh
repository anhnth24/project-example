#!/usr/bin/env bash
# Set the seeded POC administrator password from an Argon2id PHC hash on stdin.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=poc-compose.sh
source "$ROOT/deploy/scripts/poc-compose.sh"
poc_compose_init

if [[ -t 0 ]]; then
  echo "pipe one Argon2id PHC hash to stdin" >&2
  exit 1
fi

IFS= read -r password_hash || {
  echo "missing Argon2id PHC hash on stdin" >&2
  exit 1
}
if IFS= read -r _extra; then
  echo "expected exactly one hash line" >&2
  exit 1
fi
if [[ ! "$password_hash" =~ ^\$argon2id\$v=19\$m=[0-9]+,t=[0-9]+,p=[0-9]+\$[A-Za-z0-9+/]+\$[A-Za-z0-9+/]+$ ]]; then
  echo "invalid Argon2id PHC hash" >&2
  exit 1
fi

admin_email=admin@poc.example
psql_args=(
  exec -T postgres psql
  -U "${MARKHAND_POSTGRES_USER:-markhand}"
  -d "${MARKHAND_POSTGRES_DB:-markhand}"
  --set ON_ERROR_STOP=1
)

row_count="$(
  "${COMPOSE[@]}" "${psql_args[@]}" -tAc \
    "SELECT count(*) FROM users WHERE email = '${admin_email}';"
)"
row_count="${row_count//[[:space:]]/}"
if [[ "$row_count" != "1" ]]; then
  echo "seeded administrator row not found" >&2
  exit 1
fi

"${COMPOSE[@]}" "${psql_args[@]}" \
  --set "password_hash=$password_hash" \
  -c "UPDATE users
      SET password_hash = :'password_hash', updated_at = now()
      WHERE email = '${admin_email}';" \
  >/dev/null

echo "updated seeded administrator password hash"
