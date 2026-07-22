#!/usr/bin/env bash
# Restore MinIO originals/derived artifacts to at least the PostgreSQL recovery point.
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
  markhand_die "usage: restore-minio.sh <backup-root> [dry_run=1]"
fi

MANIFEST="$BACKUP_DIR/recovery-manifest.json"
"$SCRIPT_DIR/validate-manifest.sh" "$MANIFEST" "$BACKUP_DIR"

INV="$(markhand_resolve_under "$BACKUP_DIR" "minio/version-inventory.tsv")"
[[ -f "$INV" ]] || markhand_die "missing MinIO version inventory artifact"
MIRROR="$BACKUP_DIR/minio/mirror"

EXPECTED="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["minio"]["inventoryDigestSha256"])' "$MANIFEST")"
ACTUAL="$(python3 "$MANIFEST_PY" inventory-digest --input "$INV" | python3 -c 'import json,sys; print(json.load(sys.stdin)["digestSha256"])')"
[[ "$EXPECTED" == "$ACTUAL" ]] || markhand_die "MinIO inventory digest mismatch"

if [[ "$DRY_RUN" == "1" ]]; then
  markhand_log "DRY-RUN restore-minio inventory_ok count_bytes=$(wc -c <"$INV")"
  exit 0
fi

markhand_require_destructive_confirm
markhand_require_env MARKHAND_BACKUP_MINIO_ENDPOINT
markhand_require_env MARKHAND_BACKUP_MINIO_ACCESS_KEY
markhand_require_env MARKHAND_BACKUP_MINIO_SECRET_KEY
markhand_require_env MARKHAND_MINIO_BUCKET
markhand_require_cmd mc

ALIAS="markhandrestore$$"
mc alias set "$ALIAS" "$MARKHAND_BACKUP_MINIO_ENDPOINT" \
  "$MARKHAND_BACKUP_MINIO_ACCESS_KEY" "$MARKHAND_BACKUP_MINIO_SECRET_KEY" >/dev/null

if [[ ! -d "$MIRROR" ]]; then
  markhand_die "missing minio/mirror directory for restore"
fi
mc mirror --overwrite --remove "$MIRROR" "$ALIAS/$MARKHAND_MINIO_BUCKET" >/dev/null \
  || markhand_die "mc mirror restore failed"

markhand_checkpoint_set "$BACKUP_DIR" "minio-restored"
markhand_log "minio restore complete bucket=$MARKHAND_MINIO_BUCKET"
