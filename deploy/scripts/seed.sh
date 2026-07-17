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
SQL

echo "seeded local development metadata"
