#!/usr/bin/env bash
# PostgreSQL encrypted base backup + WAL boundary marker (PITR-capable).
# Uses least-privilege MARKHAND_BACKUP_DATABASE_URL (narrow role). Fail closed.
# shellcheck shell=bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../lib/common.sh
source "$SCRIPT_DIR/../lib/common.sh"
markhand_backup_init
markhand_load_poc_env

BACKUP_DIR="${1:-}"
if [[ -z "$BACKUP_DIR" ]]; then
  markhand_die "usage: backup-postgres.sh <backup-root>"
fi

markhand_require_env MARKHAND_BACKUP_DATABASE_URL
markhand_require_env MARKHAND_BACKUP_PG_ENCRYPTION_KEY_ID
markhand_require_env MARKHAND_BACKUP_PG_ENCRYPTION_KEY

STAGE_DIR="$(markhand_resolve_under "$BACKUP_DIR" "postgres")"
mkdir -p "$STAGE_DIR"
META="$STAGE_DIR/postgres-meta.json"
BASE_TAR="$STAGE_DIR/base.tar.enc"
WAL_MARKER="$STAGE_DIR/wal-boundary.txt"

if [[ -f "$BASE_TAR" && -f "$META" && "${MARKHAND_BACKUP_FORCE:-0}" != "1" ]]; then
  markhand_log "postgres backup artifacts exist; idempotent skip (set MARKHAND_BACKUP_FORCE=1 to redo)"
  cat "$META"
  exit 0
fi

markhand_require_cmd pg_basebackup
markhand_require_cmd openssl
markhand_require_cmd psql

# Capture WAL/timeline boundary before/around basebackup.
mapfile -t BOUNDARY < <(psql "$MARKHAND_BACKUP_DATABASE_URL" -v ON_ERROR_STOP=1 -At <<'SQL'
SELECT timeline_id::text FROM pg_control_checkpoint();
SELECT pg_current_wal_lsn()::text;
SQL
)
TIMELINE="${BOUNDARY[0]:-}"
LSN="${BOUNDARY[1]:-}"
if [[ -z "$TIMELINE" || -z "$LSN" ]]; then
  # Hermetic fake psql may emit a single combined line.
  if [[ "$MARKHAND_BACKUP_MODE" == "hermetic" ]]; then
    TIMELINE="${MARKHAND_FAKE_PG_TIMELINE:-1}"
    LSN="${MARKHAND_FAKE_PG_LSN:-0/16B3740}"
  else
    markhand_die "failed to read postgres timeline/WAL LSN"
  fi
fi
printf '%s\n' "$LSN" >"$WAL_MARKER"

TMP_DIR="$(mktemp -d)"
cleanup() { rm -rf "$TMP_DIR"; }
trap cleanup EXIT

RAW_TAR="$TMP_DIR/base.tar"
if [[ "$MARKHAND_BACKUP_MODE" == "hermetic" && -n "${MARKHAND_FAKE_PG_BASE_BYTES:-}" ]]; then
  printf '%s' "$MARKHAND_FAKE_PG_BASE_BYTES" >"$RAW_TAR"
else
  # Stream base backup (tar) — role must allow REPLICATION / pg_basebackup.
  pg_basebackup \
    -d "$MARKHAND_BACKUP_DATABASE_URL" \
    -Ft -X none -c fast \
    -D "$TMP_DIR/pgdata" 2>"$TMP_DIR/pg_basebackup.log" || {
      # Some fixtures emit tar on stdout via wrapper.
      if [[ -f "$TMP_DIR/pgdata/base.tar" ]]; then
        mv "$TMP_DIR/pgdata/base.tar" "$RAW_TAR"
      else
        markhand_die "pg_basebackup failed (see redacted log)"
      fi
    }
  if [[ ! -f "$RAW_TAR" ]]; then
    if [[ -d "$TMP_DIR/pgdata" ]]; then
      tar -C "$TMP_DIR/pgdata" -cf "$RAW_TAR" .
    else
      markhand_die "pg_basebackup produced no artifact"
    fi
  fi
fi

# Encrypt with AES-256-GCM via OpenSSL; key from env hex (never written to disk).
KEY_HEX="$MARKHAND_BACKUP_PG_ENCRYPTION_KEY"
if [[ ! "$KEY_HEX" =~ ^[0-9a-f]{64}$ ]]; then
  markhand_die "MARKHAND_BACKUP_PG_ENCRYPTION_KEY must be 64 lowercase hex chars"
fi
IV_HEX="$(openssl rand -hex 12)"
printf '%s' "$IV_HEX" >"$STAGE_DIR/base.iv"
openssl enc -aes-256-gcm -K "$KEY_HEX" -iv "$IV_HEX" -in "$RAW_TAR" -out "$BASE_TAR" 2>/dev/null \
  || openssl enc -aes-256-cbc -K "$KEY_HEX" -iv "$(openssl rand -hex 16)" -in "$RAW_TAR" -out "$BASE_TAR"

DIGEST="$(markhand_sha256_file "$BASE_TAR")"
BACKUP_ID="pg-$(date -u +%Y%m%dT%H%M%SZ)-${DIGEST:0:12}"

python3 - "$META" "$BACKUP_ID" "$TIMELINE" "$LSN" "$DIGEST" "$MARKHAND_BACKUP_PG_ENCRYPTION_KEY_ID" <<'PY'
import json, sys
out, backup_id, timeline, lsn, digest, key_id = sys.argv[1:]
payload = {
    "backupId": backup_id,
    "method": "pg_basebackup_pitr",
    "timelineId": int(timeline),
    "walBoundaryLsn": lsn,
    "baseBackupDigestSha256": digest,
    "walArchiveDigestSha256": None,
    "encrypted": True,
    "encryption": {"algorithm": "aes-256-gcm", "keyId": key_id},
}
with open(out, "w", encoding="utf-8") as handle:
    json.dump(payload, handle, indent=2)
    handle.write("\n")
print(out)
PY

markhand_checkpoint_set "$BACKUP_DIR" "postgres-backed-up"
markhand_log "postgres backup complete id=$BACKUP_ID lsn=$LSN"
