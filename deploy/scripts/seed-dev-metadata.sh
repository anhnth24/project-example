#!/usr/bin/env bash
# Record dev fixture UUIDs in markhand_dev_seed (after migrations).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
COMPOSE=(docker compose -f "$ROOT/deploy/dev/compose.yml")
PASSWORD="${MARKHAND_DEV_PASSWORD:-markhand-dev}"
PASSWORD_SQL="${PASSWORD//\'/\'\'}"

"${COMPOSE[@]}" exec -T postgres psql \
  -U "${MARKHAND_POSTGRES_USER:-markhand}" \
  -d "${MARKHAND_POSTGRES_DB:-markhand}" \
  --set ON_ERROR_STOP=1 <<SQL
CREATE TABLE IF NOT EXISTS markhand_dev_seed (
  key text PRIMARY KEY,
  value text NOT NULL
);

INSERT INTO markhand_dev_seed (key, value) VALUES
  ('environment', 'local-dev'),
  ('seed_version', '1'),
  ('default_login_password', '${PASSWORD_SQL}'),
  ('login_email_admin', 'admin@poc.example'),
  ('login_email_owner', 'owner@example.test'),
  ('poc_org_id', '11111111-1111-1111-1111-111111111111'),
  ('poc_admin_user_id', '22222222-2222-2222-2222-222222222201'),
  ('poc_collection_id', '55555555-5555-5555-5555-555555555501'),
  ('api_bind', '127.0.0.1:8787'),
  ('mock_embedding_url', 'http://127.0.0.1:8088/v1'),
  ('embedding_model', 'AITeamVN/Vietnamese_Embedding'),
  ('embedding_revision', 'dea33aa1ab339f38d66ae0a40e6c40e0a9249568'),
  ('embedding_dimensions', '1024'),
  ('index_signature_aiteamvn', 'ca03085c08f4c01d391ac973192815c944892f6e74b52e7bf4e1f135f65ae97c')
ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value;

-- Prefer live org id when slug poc exists (migration 0011 or seed-poc-org).
UPDATE markhand_dev_seed AS seed
SET value = orgs.id::text
FROM orgs
WHERE seed.key = 'poc_org_id' AND orgs.slug = 'poc';
SQL

echo "seeded dev metadata (markhand_dev_seed)"
