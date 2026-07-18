#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
COMPOSE=(docker compose -f "$ROOT/deploy/dev/compose.yml")

"${COMPOSE[@]}" exec -T postgres psql \
  -U "${MARKHAND_POSTGRES_USER:-markhand}" \
  -d "${MARKHAND_POSTGRES_DB:-markhand}" \
  --set ON_ERROR_STOP=1 <<'SQL'
CREATE TABLE IF NOT EXISTS markhand_dev_seed (
  key text PRIMARY KEY,
  value text NOT NULL
);
INSERT INTO markhand_dev_seed (key, value)
VALUES ('environment', 'local-dev')
ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value;

INSERT INTO orgs (slug, name)
VALUES ('poc', 'Markhand POC')
ON CONFLICT (slug) DO UPDATE SET name = EXCLUDED.name
RETURNING id AS poc_org_id \gset

INSERT INTO users (email, display_name)
VALUES ('owner@example.test', 'POC Owner')
ON CONFLICT (email) DO UPDATE SET display_name = EXCLUDED.display_name
RETURNING id AS poc_user_id \gset

INSERT INTO org_memberships (org_id, user_id, role)
VALUES (:'poc_org_id', :'poc_user_id', 'owner')
ON CONFLICT (org_id, user_id) DO UPDATE SET role = EXCLUDED.role;
SQL

echo "seeded local development metadata and POC organization"
