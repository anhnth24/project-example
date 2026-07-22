#!/usr/bin/env bash
# Default dry-run; --apply requires MARKHAND_RESTORE_CONFIRM.
# shellcheck shell=bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
BACKUP_ROOT="${1:?usage: restore.sh <backup-root> [--apply]}"
shift || true
ARGS=(restore --backup-root "$BACKUP_ROOT")
if [[ "${1:-}" == "--apply" ]]; then
  ARGS+=(--apply)
fi
if [[ -n "${MARKHAND_RESTORE_TARGET_STATE:-}" ]]; then
  ARGS+=(--target-state "$MARKHAND_RESTORE_TARGET_STATE")
fi
exec python3 "$ROOT/deploy/backup/lib/pipeline.py" "${ARGS[@]}"
