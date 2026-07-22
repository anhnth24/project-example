#!/usr/bin/env bash
# Create/download a Qdrant snapshot and bind generation + index signature.
# shellcheck shell=bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../lib/common.sh
source "$SCRIPT_DIR/../lib/common.sh"
markhand_backup_init
markhand_load_poc_env

BACKUP_DIR="${1:-}"
if [[ -z "$BACKUP_DIR" ]]; then
  markhand_die "usage: backup-qdrant.sh <backup-root>"
fi

markhand_require_env MARKHAND_BACKUP_QDRANT_URL
markhand_require_env MARKHAND_INDEX_SIGNATURE
markhand_require_env MARKHAND_BACKUP_QDRANT_COLLECTION

STAGE_DIR="$(markhand_resolve_under "$BACKUP_DIR" "qdrant")"
mkdir -p "$STAGE_DIR"
META="$STAGE_DIR/qdrant-meta.json"
SNAP_FILE="$STAGE_DIR/snapshot.bin"

if [[ -f "$META" && -f "$SNAP_FILE" && "${MARKHAND_BACKUP_FORCE:-0}" != "1" ]]; then
  markhand_log "qdrant snapshot exists; idempotent skip"
  cat "$META"
  exit 0
fi

markhand_require_cmd curl
markhand_require_cmd jq

BASE="${MARKHAND_BACKUP_QDRANT_URL%/}"
COLLECTION="$MARKHAND_BACKUP_QDRANT_COLLECTION"
AUTH_ARGS=()
if [[ -n "${MARKHAND_BACKUP_QDRANT_API_KEY:-}" ]]; then
  AUTH_ARGS=(-H "api-key: ${MARKHAND_BACKUP_QDRANT_API_KEY}")
fi

if [[ "$MARKHAND_BACKUP_MODE" == "hermetic" && -n "${MARKHAND_FAKE_QDRANT_SNAPSHOT_BYTES:-}" ]]; then
  printf '%s' "$MARKHAND_FAKE_QDRANT_SNAPSHOT_BYTES" >"$SNAP_FILE"
  SNAPSHOT_ID="${MARKHAND_FAKE_QDRANT_SNAPSHOT_ID:-snap-hermetic-001}"
  GENERATION="${MARKHAND_FAKE_QDRANT_GENERATION:-1}"
else
  CREATE_JSON="$(curl -fsS "${AUTH_ARGS[@]}" -X POST \
    "$BASE/collections/$COLLECTION/snapshots")"
  SNAPSHOT_ID="$(jq -r '.result.name // .result // empty' <<<"$CREATE_JSON")"
  if [[ -z "$SNAPSHOT_ID" || "$SNAPSHOT_ID" == "null" ]]; then
    markhand_die "qdrant snapshot create failed / missing name"
  fi
  curl -fsS "${AUTH_ARGS[@]}" \
    "$BASE/collections/$COLLECTION/snapshots/$SNAPSHOT_ID" \
    -o "$SNAP_FILE"
  GENERATION="$(curl -fsS "${AUTH_ARGS[@]}" "$BASE/collections/$COLLECTION" \
    | jq -r '.result.status // .result // 0' 2>/dev/null || echo 0)"
  if ! [[ "$GENERATION" =~ ^[0-9]+$ ]]; then
    GENERATION="${MARKHAND_BACKUP_QDRANT_GENERATION:-0}"
  fi
fi

DIGEST="$(markhand_sha256_file "$SNAP_FILE")"
CAPTURED="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

python3 - "$META" "$SNAPSHOT_ID" "$GENERATION" "$MARKHAND_INDEX_SIGNATURE" "$DIGEST" "$CAPTURED" <<'PY'
import json, sys
out, snap_id, generation, sig, digest, captured = sys.argv[1:]
payload = {
    "snapshotId": snap_id,
    "collectionGeneration": int(generation),
    "indexSignatureSha256": sig,
    "snapshotDigestSha256": digest,
    "capturedAt": captured,
}
with open(out, "w", encoding="utf-8") as handle:
    json.dump(payload, handle, indent=2)
    handle.write("\n")
print(out)
PY

markhand_checkpoint_set "$BACKUP_DIR" "qdrant-backed-up"
markhand_log "qdrant snapshot id=$SNAPSHOT_ID digest=${DIGEST:0:12}…"
