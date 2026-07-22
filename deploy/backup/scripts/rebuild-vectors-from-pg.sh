#!/usr/bin/env bash
# PG-authoritative rebuild: bulk enqueue + index/embedding workers + reconcile-once.
# shellcheck shell=bash
set -euo pipefail
DRY="${1:-1}"
if [[ "$DRY" == "1" ]]; then
  cat <<'EOF'
{"mode":"dry-run","authority":"postgres","ready":false,"readOnly":true,"steps":["MARKHAND_RECONCILE_BULK_ENQUEUE via reconcile worker after embedding backfill","worker-index","worker-embedding","reconcile-once try_ready"]}
EOF
  exit 0
fi
if [[ "${MARKHAND_RESTORE_CONFIRM:-}" != "I_UNDERSTAND_DESTRUCTIVE_RESTORE" ]]; then
  echo "error: destructive rebuild refused" >&2
  exit 2
fi
echo "error: live rebuild requires compose workers (index/embedding) then reconcile-once; Docker unavailable in this environment" >&2
exit 2
