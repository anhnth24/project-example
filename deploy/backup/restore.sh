#!/usr/bin/env bash
# P1B-O03 restore-green only — promote/cutover disabled (no false traffic switch).
set -euo pipefail
umask 077
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BACKUP_DIR="${1:?usage: restore.sh <backup-dir>}"
: "${DATABASE_URL:?DATABASE_URL required}"
: "${MARKHAND_BACKUP_SIGNING_KEY:?MARKHAND_BACKUP_SIGNING_KEY required}"
: "${MARKHAND_BACKUP_KEY_ID:?MARKHAND_BACKUP_KEY_ID required}"

export PYTHONPATH="$ROOT/deploy/backup/lib${PYTHONPATH:+:$PYTHONPATH}"

for arg in "$@"; do
  if [[ -n "$MARKHAND_BACKUP_SIGNING_KEY" && "$arg" == *"$MARKHAND_BACKUP_SIGNING_KEY"* ]]; then
    echo "signing key must not appear on argv" >&2
    exit 1
  fi
done

# Fail closed before any mutate if green targets missing.
if [[ -z "${MARKHAND_GREEN_DATABASE_URL:-}" || -z "${MARKHAND_GREEN_MINIO_BUCKET:-}" || -z "${MARKHAND_GREEN_QDRANT_COLLECTION:-}" ]]; then
  echo "blue/green restore requires isolated green targets" >&2
  echo "REFUSING_DESTRUCTIVE_PROMOTE" >&2
  exit 2
fi
if [[ -z "${MARKHAND_GREEN_ALLOWLIST_JSON:-}" ]]; then
  echo "MARKHAND_GREEN_ALLOWLIST_JSON required before restore" >&2
  exit 1
fi
if [[ -z "${MARKHAND_GREEN_MINIO_ALLOWLIST_JSON:-}" ]]; then
  echo "MARKHAND_GREEN_MINIO_ALLOWLIST_JSON required (mandatory allowlist policy)" >&2
  exit 1
fi
if [[ -z "${MARKHAND_GREEN_QDRANT_ALLOWLIST_JSON:-}" ]]; then
  echo "MARKHAND_GREEN_QDRANT_ALLOWLIST_JSON required (mandatory allowlist policy)" >&2
  exit 1
fi

# Promote/cutover refused before mutation — no partial traffic switch.
if [[ "${MARKHAND_RESTORE_CUTOVER:-}" == "1" || "${MARKHAND_RESTORE_PROMOTE:-}" == "1" ]]; then
  echo "PROMOTE_DISABLED_UNTIL_API_CONSUMES_ROUTING_AND_INDEPENDENT_DURABLE_RECONCILE_TARGET_STATE_ATTESTATION" >&2
  exit 3
fi

command -v psql >/dev/null || { echo "psql required" >&2; exit 1; }
command -v pg_restore >/dev/null || { echo "pg_restore required" >&2; exit 1; }
command -v mc >/dev/null || { echo "mc required" >&2; exit 1; }

python3 "$ROOT/deploy/backup/lib/pipeline.py" restore-green "$BACKUP_DIR"
echo "RESTORE_GREEN_OK_PROMOTE_DISABLED"
