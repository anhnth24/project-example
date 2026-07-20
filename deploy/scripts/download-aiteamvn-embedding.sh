#!/usr/bin/env bash
# Prefetch AITeamVN weights into the Compose HuggingFace cache volume (optional, dev).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
COMPOSE=(docker compose -f "$ROOT/deploy/dev/compose.yml")

export COMPOSE_PROFILES="${COMPOSE_PROFILES:-aiteamvn}"

"${COMPOSE[@]}" build embedding-cpu
"${COMPOSE[@]}" run --rm --no-deps embedding-cpu python - <<'PY'
import os
from sentence_transformers import SentenceTransformer

hub = os.environ["MARKHAND_EMBEDDING_HUB_ID"]
revision = os.environ["MARKHAND_EMBEDDING_REVISION"]
print(f"downloading {hub}@{revision} ...")
SentenceTransformer(hub, revision=revision, device="cpu")
print("download complete")
PY

echo "model cached in Docker volume embedding_model_cache"
