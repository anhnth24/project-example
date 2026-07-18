#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
COMPOSE=(docker compose -f "$ROOT/deploy/dev/compose.yml")

"${COMPOSE[@]}" exec -T postgres psql \
  -U "${MARKHAND_POSTGRES_USER:-markhand}" \
  -d "${MARKHAND_POSTGRES_DB:-markhand}" \
  --set ON_ERROR_STOP=1 <<'SQL'
SELECT to_regclass('public.org_memberships') AS memberships_table \gset
\if :{?memberships_table}
\else
\quit 3
\endif

INSERT INTO orgs (slug, name)
VALUES ('poc', 'Markhand POC')
ON CONFLICT (slug) DO UPDATE SET name = EXCLUDED.name, updated_at = now()
RETURNING id AS poc_org_id \gset

INSERT INTO users (email, display_name)
VALUES ('owner@example.test', 'POC Owner')
ON CONFLICT (email) DO UPDATE SET display_name = EXCLUDED.display_name, updated_at = now()
RETURNING id AS poc_user_id \gset

INSERT INTO org_memberships (org_id, user_id, role)
VALUES (:'poc_org_id', :'poc_user_id', 'owner')
ON CONFLICT (org_id, user_id) DO UPDATE SET role = EXCLUDED.role;
SQL

echo "seeded POC organization"
