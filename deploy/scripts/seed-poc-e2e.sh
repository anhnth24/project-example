#!/usr/bin/env bash
# Seed synthetic E2E accounts/org-B against a tagged test compose stack (P1B-O04).
# Fail-closed: requires exact destructive confirmation + e2e/test tags.
# Never defaults to human `markhand` project/db/bucket names.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CONFIRM_PHRASE="i-understand-this-mutates-only-tagged-test-stacks"

die() { echo "FATAL: $*" >&2; exit 1; }

[[ "${MARKHAND_E2E_CONFIRM:-}" == "$CONFIRM_PHRASE" ]] \
  || die "seed-poc-e2e.sh requires MARKHAND_E2E_CONFIRM=$CONFIRM_PHRASE"

# Dedicated E2E defaults only — never fall back to human `markhand`.
export MARKHAND_COMPOSE_PROJECT="${MARKHAND_COMPOSE_PROJECT:-markhand-e2e}"
export MARKHAND_POSTGRES_DB="${MARKHAND_POSTGRES_DB:-markhand_e2e}"
export MARKHAND_MINIO_BUCKET="${MARKHAND_MINIO_BUCKET:-markhand-e2e-documents}"
export MARKHAND_E2E_STACK_TAG="${MARKHAND_E2E_STACK_TAG:-test}"
export MARKHAND_POSTGRES_USER="${MARKHAND_POSTGRES_USER:-markhand_e2e}"

project="${MARKHAND_COMPOSE_PROJECT}"
db="${MARKHAND_POSTGRES_DB}"
bucket="${MARKHAND_MINIO_BUCKET}"
tag="${MARKHAND_E2E_STACK_TAG}"

[[ "$project" =~ [Ee]2[Ee]|[Tt][Ee][Ss][Tt] ]] \
  || die "MARKHAND_COMPOSE_PROJECT must contain e2e/test (got '$project')"
[[ "$db" =~ [Ee]2[Ee]|[Tt][Ee][Ss][Tt] ]] \
  || die "MARKHAND_POSTGRES_DB must contain e2e/test (got '$db')"
[[ "$bucket" =~ [Ee]2[Ee]|[Tt][Ee][Ss][Tt] ]] \
  || die "MARKHAND_MINIO_BUCKET must contain e2e/test (got '$bucket')"
[[ "$tag" == "test" ]] || die "MARKHAND_E2E_STACK_TAG must be 'test'"

# Explicitly refuse untagged human defaults even if somehow injected.
[[ "$project" != "markhand" && "$project" != "markhand-poc" ]] \
  || die "refusing untagged/human compose project '$project'"
[[ "$db" != "markhand" ]] || die "refusing human postgres db 'markhand'"
[[ "$bucket" != "markhand-documents" ]] || die "refusing human minio bucket 'markhand-documents'"

# shellcheck source=poc-compose.sh
source "$ROOT/deploy/scripts/poc-compose.sh"
poc_compose_init

SQL="$ROOT/crates/server/tests/e2e/sql/seed_e2e_accounts.sql"
PASSWORD="${MARKHAND_E2E_PASSWORD:-${MARKHAND_DEV_PASSWORD:-markhand-e2e}}"

if [[ ! -f "$SQL" ]]; then
  die "missing $SQL"
fi

echo "seeding E2E accounts into project=$project db=$db ..."
"${COMPOSE[@]}" exec -T postgres psql \
  -U "${MARKHAND_POSTGRES_USER}" \
  -d "${MARKHAND_POSTGRES_DB}" \
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
    -U "${MARKHAND_POSTGRES_USER}" \
    -d "${MARKHAND_POSTGRES_DB}" \
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
  die "no passwords updated — ensure migrations applied and API started once"
fi

# Export foreign seeded IDs for IDOR matrix (also documented here for runners).
export MARKHAND_E2E_FOREIGN_DOCUMENT_ID="${MARKHAND_E2E_FOREIGN_DOCUMENT_ID:-67676767-6767-4676-8676-676767676701}"
export MARKHAND_E2E_FOREIGN_VERSION_ID="${MARKHAND_E2E_FOREIGN_VERSION_ID:-68686868-6868-4686-8686-686868686801}"

echo "E2E seed complete (password via MARKHAND_E2E_PASSWORD; not printed)"
echo "foreign document id set for IDOR matrix"
