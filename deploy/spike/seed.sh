#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "$0")/common.sh"

"${COMPOSE[@]}" exec -T postgres psql \
  -U "$MARKHAND_POSTGRES_USER" \
  -d "$MARKHAND_POSTGRES_DB" \
  --set ON_ERROR_STOP=1 <<'SQL'
CREATE EXTENSION IF NOT EXISTS unaccent;
CREATE TABLE IF NOT EXISTS markhand_spike_seed (
  key text PRIMARY KEY,
  value text NOT NULL
);
INSERT INTO markhand_spike_seed (key, value)
VALUES
  ('environment', 'benchmark-spike'),
  ('workload_profile', 'on-prem-reference-v1')
ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value;
SQL

curl --fail --silent --show-error \
  -X PUT "http://127.0.0.1:${MARKHAND_SPIKE_QDRANT_HTTP_PORT}/collections/markhand_spike_smoke" \
  -H "content-type: application/json" \
  --data "{\"vectors\":{\"size\":${MARKHAND_SPIKE_MOCK_DIMENSIONS},\"distance\":\"Cosine\"}}" \
  >/dev/null

echo "seeded benchmark spike metadata and Qdrant smoke collection"
