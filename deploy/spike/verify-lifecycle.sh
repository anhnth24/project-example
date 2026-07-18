#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "$0")/common.sh"

"${COMPOSE[@]}" exec -T postgres psql \
  -U "$MARKHAND_POSTGRES_USER" \
  -d "$MARKHAND_POSTGRES_DB" \
  --set ON_ERROR_STOP=1 \
  -c "INSERT INTO markhand_spike_seed(key,value) VALUES ('lifecycle_sentinel','persist') ON CONFLICT (key) DO UPDATE SET value=EXCLUDED.value"

payload="$(python3 - "$MARKHAND_SPIKE_MOCK_DIMENSIONS" <<'PY'
import json
import sys
dimensions = int(sys.argv[1])
print(json.dumps({"points": [{"id": 4242, "vector": [0.0] * dimensions}]}))
PY
)"
curl --fail --silent --show-error \
  -X PUT "http://127.0.0.1:${MARKHAND_SPIKE_QDRANT_HTTP_PORT}/collections/markhand_spike_smoke/points?wait=true" \
  -H "content-type: application/json" \
  --data "$payload" >/dev/null

"$SPIKE_DIR/down.sh"
"$SPIKE_DIR/up.sh"

persisted="$("${COMPOSE[@]}" exec -T postgres psql \
  -U "$MARKHAND_POSTGRES_USER" \
  -d "$MARKHAND_POSTGRES_DB" \
  -Atc "SELECT value FROM markhand_spike_seed WHERE key='lifecycle_sentinel'")"
[[ "$persisted" == "persist" ]] || {
  echo "PostgreSQL sentinel did not survive restart" >&2
  exit 1
}
curl --fail --silent --show-error \
  "http://127.0.0.1:${MARKHAND_SPIKE_QDRANT_HTTP_PORT}/collections/markhand_spike_smoke/points/4242" \
  >/dev/null

"$SPIKE_DIR/reset.sh"

remaining="$("${COMPOSE[@]}" exec -T postgres psql \
  -U "$MARKHAND_POSTGRES_USER" \
  -d "$MARKHAND_POSTGRES_DB" \
  -Atc "SELECT count(*) FROM markhand_spike_seed WHERE key='lifecycle_sentinel'")"
[[ "$remaining" == "0" ]] || {
  echo "PostgreSQL sentinel survived volume reset" >&2
  exit 1
}
if curl --fail --silent \
  "http://127.0.0.1:${MARKHAND_SPIKE_QDRANT_HTTP_PORT}/collections/markhand_spike_smoke/points/4242" \
  >/dev/null; then
  echo "Qdrant sentinel survived volume reset" >&2
  exit 1
fi

"$SPIKE_DIR/seed.sh"
echo "spike lifecycle verified: restart preserves data, reset removes sentinels"
