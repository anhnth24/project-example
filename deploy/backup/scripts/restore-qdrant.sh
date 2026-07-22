#!/usr/bin/env bash
# Restore matching Qdrant snapshot, or refuse and point operators at PG rebuild.
# shellcheck shell=bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../lib/common.sh
source "$SCRIPT_DIR/../lib/common.sh"
markhand_backup_init
markhand_load_poc_env

BACKUP_DIR="${1:-}"
DRY_RUN="${2:-1}"
if [[ -z "$BACKUP_DIR" ]]; then
  markhand_die "usage: restore-qdrant.sh <backup-root> [dry_run=1]"
fi

MANIFEST="$BACKUP_DIR/recovery-manifest.json"
"$SCRIPT_DIR/validate-manifest.sh" "$MANIFEST" "$BACKUP_DIR"

SNAP="$(markhand_resolve_under "$BACKUP_DIR" "qdrant/snapshot.bin")"
[[ -f "$SNAP" ]] || markhand_die "missing Qdrant snapshot artifact"

EXPECTED="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["qdrant"]["snapshotDigestSha256"])' "$MANIFEST")"
ACTUAL="$(markhand_sha256_file "$SNAP")"
[[ "$EXPECTED" == "$ACTUAL" ]] || markhand_die "Qdrant snapshot digest mismatch"

SIG_MANIFEST="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["qdrant"]["indexSignatureSha256"])' "$MANIFEST")"
[[ "$SIG_MANIFEST" == "${MARKHAND_INDEX_SIGNATURE:-}" ]] \
  || markhand_die "index signature mismatch vs MARKHAND_INDEX_SIGNATURE"

if [[ "$DRY_RUN" == "1" ]]; then
  markhand_log "DRY-RUN restore-qdrant snapshot_ok"
  exit 0
fi

markhand_require_destructive_confirm
markhand_require_env MARKHAND_BACKUP_QDRANT_URL
markhand_require_env MARKHAND_BACKUP_QDRANT_COLLECTION
markhand_require_cmd curl

BASE="${MARKHAND_BACKUP_QDRANT_URL%/}"
COLLECTION="$MARKHAND_BACKUP_QDRANT_COLLECTION"
AUTH_ARGS=()
if [[ -n "${MARKHAND_BACKUP_QDRANT_API_KEY:-}" ]]; then
  AUTH_ARGS=(-H "api-key: ${MARKHAND_BACKUP_QDRANT_API_KEY}")
fi

# Upload snapshot bytes then recover. Exact API varies by Qdrant version; pinned
# image is qdrant v1.18.2 (see deploy/poc/images.lock.json).
SNAP_ID="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["qdrant"]["snapshotId"])' "$MANIFEST")"
curl -fsS "${AUTH_ARGS[@]}" -X POST \
  -H "Content-Type: application/octet-stream" \
  --data-binary @"$SNAP" \
  "$BASE/collections/$COLLECTION/snapshots/upload?snapshot_name=$SNAP_ID" >/dev/null \
  || markhand_die "qdrant snapshot upload failed"
curl -fsS "${AUTH_ARGS[@]}" -X PUT \
  "$BASE/collections/$COLLECTION/snapshots/$SNAP_ID/recover" >/dev/null \
  || markhand_die "qdrant snapshot recover failed"

markhand_checkpoint_set "$BACKUP_DIR" "qdrant-restored"
markhand_log "qdrant snapshot restored id=$SNAP_ID"
