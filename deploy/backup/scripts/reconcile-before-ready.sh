#!/usr/bin/env bash
# Live path: fileconv-worker reconcile bulk+once. Hermetic: restore --apply records outcomes.
# shellcheck shell=bash
set -euo pipefail
MODE="${1:-detect}"
DRY="${2:-1}"
if [[ "$DRY" == "1" ]]; then
  echo '{"mode":"'"$MODE"'","dryRun":true,"ready":false,"readOnly":true}'
  exit 0
fi
if [[ "${MARKHAND_RESTORE_CONFIRM:-}" != "I_UNDERSTAND_DESTRUCTIVE_RESTORE" ]]; then
  echo "error: destructive reconcile refused" >&2
  exit 2
fi
if [[ "${MARKHAND_BACKUP_MODE:-}" == "hermetic" ]]; then
  echo "error: hermetic reconcile is performed inside restore --apply" >&2
  exit 2
fi
echo "error: run fileconv-worker MARKHAND_WORKER_KIND=reconcile MARKHAND_RECONCILE_BULK_ENQUEUE=1 MARKHAND_RECONCILE_ONCE=1 MARKHAND_RECONCILE_MODE=${MODE}" >&2
exit 2
