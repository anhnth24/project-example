#!/usr/bin/env bash
# Restore PostgreSQL from encrypted base backup to the manifest WAL boundary.
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
  markhand_die "usage: restore-postgres.sh <backup-root> [dry_run=1]"
fi

MANIFEST="$BACKUP_DIR/recovery-manifest.json"
[[ -f "$MANIFEST" ]] || markhand_die "missing recovery manifest"
"$SCRIPT_DIR/validate-manifest.sh" "$MANIFEST" "$BACKUP_DIR"

BASE_ENC="$(markhand_resolve_under "$BACKUP_DIR" "postgres/base.tar.enc")"
[[ -f "$BASE_ENC" ]] || markhand_die "missing postgres base backup artifact"
WAL_FILE="$(markhand_resolve_under "$BACKUP_DIR" "postgres/wal-boundary.txt")"
[[ -f "$WAL_FILE" ]] || markhand_die "missing WAL boundary artifact"

TARGET_LSN="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["postgres"]["walBoundaryLsn"])' "$MANIFEST")"
FILE_LSN="$(tr -d '[:space:]' <"$WAL_FILE")"
[[ "$TARGET_LSN" == "$FILE_LSN" ]] || markhand_die "WAL boundary mismatch manifest vs artifact"

if [[ "$DRY_RUN" == "1" ]]; then
  markhand_log "DRY-RUN restore-postgres target_lsn=$TARGET_LSN (no changes)"
  exit 0
fi

markhand_require_destructive_confirm
markhand_require_env MARKHAND_BACKUP_PG_ENCRYPTION_KEY
markhand_require_env MARKHAND_RESTORE_PGDATA
markhand_require_cmd openssl
markhand_require_cmd tar

if [[ -e "$MARKHAND_RESTORE_PGDATA" && "${MARKHAND_RESTORE_ALLOW_NONEMPTY_PGDATA:-0}" != "1" ]]; then
  markhand_die "refuse to overwrite non-empty PGDATA without MARKHAND_RESTORE_ALLOW_NONEMPTY_PGDATA=1"
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
KEY_HEX="$MARKHAND_BACKUP_PG_ENCRYPTION_KEY"
IV_FILE="$BACKUP_DIR/postgres/base.iv"
if [[ -f "$IV_FILE" ]]; then
  IV_HEX="$(tr -d '[:space:]' <"$IV_FILE")"
  openssl enc -d -aes-256-gcm -K "$KEY_HEX" -iv "$IV_HEX" -in "$BASE_ENC" -out "$TMP/base.tar" 2>/dev/null \
    || openssl enc -d -aes-256-cbc -K "$KEY_HEX" -iv "$IV_HEX" -in "$BASE_ENC" -out "$TMP/base.tar"
else
  markhand_die "missing IV file for encrypted base backup"
fi

mkdir -p "$MARKHAND_RESTORE_PGDATA"
tar -C "$MARKHAND_RESTORE_PGDATA" -xf "$TMP/base.tar"
# recovery.signal / standby.signal for PITR to TARGET_LSN is operator-local;
# hermetic mode records intent only.
printf 'restore_command = unset\nrecovery_target_lsn = %s\n' "$TARGET_LSN" \
  >"$MARKHAND_RESTORE_PGDATA/markhand-recovery.conf"
markhand_checkpoint_set "$BACKUP_DIR" "postgres-restored"
markhand_log "postgres restored to LSN $TARGET_LSN (migrations forward-only after boot)"
