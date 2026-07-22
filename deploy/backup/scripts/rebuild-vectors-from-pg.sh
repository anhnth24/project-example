#!/usr/bin/env bash
# PG-authoritative vector rebuild path when Qdrant snapshot is missing/corrupt.
# Does not claim query-ready until reconcile-before-ready certifies.
# shellcheck shell=bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../lib/common.sh
source "$SCRIPT_DIR/../lib/common.sh"
markhand_backup_init
markhand_load_poc_env

DRY_RUN="${1:-1}"

markhand_require_env MARKHAND_INDEX_SIGNATURE
markhand_require_env MARKHAND_BACKUP_DATABASE_URL

REPORT_DIR="${MARKHAND_RESTORE_REPORT_DIR:-/tmp/markhand-vector-rebuild}"
mkdir -p "$REPORT_DIR"

markhand_log "PG-only vector rebuild; index_signature=${MARKHAND_INDEX_SIGNATURE:0:12}…"

# Keep readiness blocked for the duration of rebuild.
if [[ "$MARKHAND_BACKUP_MODE" == "hermetic" ]]; then
  markhand_runtime_readiness_sql open "pg vector rebuild fence" >"$REPORT_DIR/open.sql"
else
  markhand_psql -c "$(markhand_runtime_readiness_sql open "pg vector rebuild fence")" \
    | markhand_redact_line >/dev/null
fi

if [[ "$DRY_RUN" == "1" ]]; then
  cat >"$REPORT_DIR/rebuild-plan.json" <<EOF
{
  "mode": "dry-run",
  "authority": "postgres",
  "indexSignatureSha256": "$MARKHAND_INDEX_SIGNATURE",
  "steps": [
    "ensure Qdrant collection for active signature",
    "enqueue embedding/index jobs for all indexed document versions",
    "run worker-embedding + worker-index",
    "reconcile detect/repair",
    "try_ready only after convergence"
  ],
  "ready": false
}
EOF
  markhand_log "DRY-RUN PG vector rebuild plan written (ready remains false)"
  cat "$REPORT_DIR/rebuild-plan.json"
  exit 0
fi

markhand_require_destructive_confirm

if [[ "$MARKHAND_BACKUP_MODE" == "hermetic" ]]; then
  cat >"$REPORT_DIR/rebuild-result.json" <<EOF
{
  "mode": "hermetic",
  "enqueuedJobs": 1,
  "indexSignatureSha256": "$MARKHAND_INDEX_SIGNATURE",
  "ready": false,
  "next": "reconcile-before-ready.sh repair 0"
}
EOF
  cat "$REPORT_DIR/rebuild-result.json"
  exit 0
fi

if ! markhand_docker_available; then
  markhand_die "live PG vector rebuild requires Docker compose (unavailable)"
fi

# shellcheck source=../../scripts/poc-compose.sh
source "$REPO_ROOT/deploy/scripts/poc-compose.sh"
poc_compose_init
"${COMPOSE[@]}" up -d worker-index worker-embedding
markhand_log "embedding/index workers started — complete with reconcile-before-ready.sh"
printf '{"workers":["worker-index","worker-embedding"],"ready":false}\n' \
  >"$REPORT_DIR/rebuild-result.json"
cat "$REPORT_DIR/rebuild-result.json"
