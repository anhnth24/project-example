#!/usr/bin/env bash
# P1B-O03 backup capture — thin wrapper over lib/pipeline.py
set -euo pipefail
umask 077
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
: "${DATABASE_URL:?DATABASE_URL required}"
: "${MINIO_ENDPOINT:?MINIO_ENDPOINT required}"
: "${MINIO_BUCKET:?MINIO_BUCKET required}"
: "${MINIO_ACCESS_KEY:?MINIO_ACCESS_KEY required}"
: "${MINIO_SECRET_KEY:?MINIO_SECRET_KEY required}"
: "${QDRANT_URL:?QDRANT_URL required}"
: "${MARKHAND_BACKUP_SIGNING_KEY:?MARKHAND_BACKUP_SIGNING_KEY required}"
: "${MARKHAND_BACKUP_KEY_ID:?MARKHAND_BACKUP_KEY_ID required}"

# Prefer explicit MARKHAND_BACKUP_DIR; otherwise external mktemp (never workspace tmp/).
if [[ -n "${MARKHAND_BACKUP_DIR:-}" ]]; then
  OUT_DIR="$MARKHAND_BACKUP_DIR"
else
  OUT_DIR="$(mktemp -d "${TMPDIR:-/tmp}/markhand-backup.XXXXXX")"
  echo "MARKHAND_BACKUP_DIR unset; using ephemeral $OUT_DIR" >&2
fi
STAMP="${MARKHAND_BACKUP_STAMP:-$(date -u +%Y%m%dT%H%M%SZ)}"
DEST="${MARKHAND_BACKUP_DEST:-$OUT_DIR/$STAMP}"
export MARKHAND_BACKUP_STAMP="$STAMP"
export MARKHAND_BACKUP_DIR="$OUT_DIR"
export PYTHONPATH="$ROOT/deploy/backup/lib${PYTHONPATH:+:$PYTHONPATH}"

command -v psql >/dev/null || { echo "psql required" >&2; exit 1; }
command -v pg_dump >/dev/null || { echo "pg_dump required" >&2; exit 1; }
command -v mc >/dev/null || { echo "mc required" >&2; exit 1; }
command -v python3 >/dev/null || { echo "python3 required" >&2; exit 1; }

for arg in "$@"; do
  if [[ -n "$MARKHAND_BACKUP_SIGNING_KEY" && "$arg" == *"$MARKHAND_BACKUP_SIGNING_KEY"* ]]; then
    echo "signing key must not appear on argv" >&2
    exit 1
  fi
done

# Propagate pipeline failures (no || true).
python3 "$ROOT/deploy/backup/lib/pipeline.py" capture "$DEST"
