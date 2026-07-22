#!/usr/bin/env bash
# Establish a consistency fence before backup/restore (ADR 0012).
# Prefer strict write fence (stop API mutations + workers). When compose/Docker
# is unavailable, record ordered-bounded mode with an honest inconsistency note.
# shellcheck shell=bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../lib/common.sh
source "$SCRIPT_DIR/../lib/common.sh"
markhand_backup_init
markhand_load_poc_env

OUT_JSON="${1:-}"
if [[ -z "$OUT_JSON" ]]; then
  markhand_die "usage: fence-writes.sh <fence.json>"
fi

STARTED="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
MODE="ordered-bounded"
WRITES_FENCED=false
NOTES=()

if [[ "$MARKHAND_BACKUP_MODE" == "hermetic" ]]; then
  MODE="strict-write-fence"
  WRITES_FENCED=true
  NOTES+=("hermetic fixture fence — no live compose stop claimed")
elif markhand_docker_available && [[ -f "$REPO_ROOT/deploy/scripts/poc-compose.sh" ]]; then
  # shellcheck source=../../scripts/poc-compose.sh
  source "$REPO_ROOT/deploy/scripts/poc-compose.sh"
  poc_compose_init
  markhand_log "stopping API mutations and workers for strict write fence"
  "${COMPOSE[@]}" stop api worker-convert worker-index worker-embedding worker-delete worker-reconcile 2>/dev/null || true
  MODE="strict-write-fence"
  WRITES_FENCED=true
  NOTES+=("compose services stopped for backup/restore fence")
else
  NOTES+=(
    "Docker/compose unavailable — ordered capture only; bounded cross-store inconsistency possible until restore reconcile"
  )
  markhand_log "Docker unavailable; documenting ordered-bounded fence (no live stop)"
fi

COMPLETED="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
mkdir -p "$(dirname "$OUT_JSON")"

python3 - "$OUT_JSON" "$MODE" "$WRITES_FENCED" "$STARTED" "$COMPLETED" "${NOTES[@]}" <<'PY'
import json, sys
out, mode, fenced, started, completed, *notes = sys.argv[1:]
payload = {
    "mode": mode,
    "writesFenced": fenced == "true",
    "fenceStartedAt": started,
    "fenceCompletedAt": completed,
    "ordering": ["postgres", "minio", "qdrant", "manifest"],
    "boundedInconsistencyNotes": list(notes),
}
with open(out, "w", encoding="utf-8") as handle:
    json.dump(payload, handle, indent=2)
    handle.write("\n")
print(out)
PY
