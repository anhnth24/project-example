#!/usr/bin/env bash
# Capture MinIO version inventory digest (no object keys in the recovery manifest).
# shellcheck shell=bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../lib/common.sh
source "$SCRIPT_DIR/../lib/common.sh"
markhand_backup_init
markhand_load_poc_env

BACKUP_DIR="${1:-}"
if [[ -z "$BACKUP_DIR" ]]; then
  markhand_die "usage: backup-minio.sh <backup-root>"
fi

markhand_require_env MARKHAND_MINIO_BUCKET
markhand_require_env MARKHAND_BACKUP_MINIO_ENDPOINT
markhand_require_env MARKHAND_BACKUP_MINIO_ACCESS_KEY
markhand_require_env MARKHAND_BACKUP_MINIO_SECRET_KEY

STAGE_DIR="$(markhand_resolve_under "$BACKUP_DIR" "minio")"
mkdir -p "$STAGE_DIR"
META="$STAGE_DIR/minio-meta.json"
INV_RAW="$STAGE_DIR/version-inventory.tsv"
INV_DIGEST_FILE="$STAGE_DIR/inventory.digest"

if [[ -f "$META" && "${MARKHAND_BACKUP_FORCE:-0}" != "1" ]]; then
  markhand_log "minio inventory exists; idempotent skip"
  cat "$META"
  exit 0
fi

markhand_require_cmd mc

ALIAS="markhandbackup$$"
# Narrow credentials — never log secret.
mc alias set "$ALIAS" "$MARKHAND_BACKUP_MINIO_ENDPOINT" \
  "$MARKHAND_BACKUP_MINIO_ACCESS_KEY" "$MARKHAND_BACKUP_MINIO_SECRET_KEY" >/dev/null

# Ensure versioning enabled (fail closed if not).
VERSION_JSON="$(mc version info "$ALIAS/$MARKHAND_MINIO_BUCKET" --json 2>/dev/null || true)"
if [[ "$MARKHAND_BACKUP_MODE" != "hermetic" ]]; then
  if ! grep -qi '"status":[[:space:]]*"Enabled"' <<<"$VERSION_JSON"; then
    markhand_die "MinIO bucket versioning is not Enabled (fail closed)"
  fi
fi

# Inventory: hash/size/version-id rows. Keep raw inventory only under backup root
# (encrypted at rest by backup volume policy); manifest stores digest+count only.
mc ls --versions --recursive "$ALIAS/$MARKHAND_MINIO_BUCKET" \
  | markhand_redact_line >"$INV_RAW" || {
    if [[ "$MARKHAND_BACKUP_MODE" == "hermetic" && -n "${MARKHAND_FAKE_MINIO_INVENTORY:-}" ]]; then
      printf '%s\n' "$MARKHAND_FAKE_MINIO_INVENTORY" >"$INV_RAW"
    else
      markhand_die "mc ls --versions failed"
    fi
  }

DIGEST_JSON="$(python3 "$MANIFEST_PY" inventory-digest --input "$INV_RAW")"
DIGEST="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["digestSha256"])' <<<"$DIGEST_JSON")"
COUNT="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["count"])' <<<"$DIGEST_JSON")"
printf '%s\n' "$DIGEST" >"$INV_DIGEST_FILE"
CAPTURED="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

# Optional: mirror versioned objects into backup root for restore (content not in manifest).
if [[ "${MARKHAND_BACKUP_MINIO_MIRROR:-1}" == "1" ]]; then
  MIRROR_DIR="$STAGE_DIR/mirror"
  mkdir -p "$MIRROR_DIR"
  mc mirror --preserve "$ALIAS/$MARKHAND_MINIO_BUCKET" "$MIRROR_DIR" >/dev/null || {
    [[ "$MARKHAND_BACKUP_MODE" == "hermetic" ]] || markhand_die "mc mirror failed"
  }
fi

python3 - "$META" "$MARKHAND_MINIO_BUCKET" "$DIGEST" "$COUNT" "$CAPTURED" <<'PY'
import json, sys
out, bucket, digest, count, captured = sys.argv[1:]
payload = {
    "bucket": bucket,
    "versioningEnabled": True,
    "inventoryDigestSha256": digest,
    "objectVersionCount": int(count),
    "capturedAt": captured,
}
with open(out, "w", encoding="utf-8") as handle:
    json.dump(payload, handle, indent=2)
    handle.write("\n")
print(out)
PY

markhand_checkpoint_set "$BACKUP_DIR" "minio-backed-up"
markhand_log "minio inventory digest=${DIGEST:0:12}… count=$COUNT"
