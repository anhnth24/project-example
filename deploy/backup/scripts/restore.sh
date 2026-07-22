#!/usr/bin/env bash
# Staged restore orchestrator (ADR 0012). Default is dry-run; destructive apply
# requires MARKHAND_RESTORE_CONFIRM=I_UNDERSTAND_DESTRUCTIVE_RESTORE.
#
# Order: validate manifest → fence → PG → MinIO → Qdrant|PG-rebuild →
# reconcile (readiness blocked) → try_ready only after convergence.
# shellcheck shell=bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../lib/common.sh
source "$SCRIPT_DIR/../lib/common.sh"
markhand_backup_init
markhand_load_poc_env

BACKUP_DIR="${1:-}"
APPLY="${2:-}"
if [[ -z "$BACKUP_DIR" ]]; then
  cat >&2 <<'USAGE'
usage: restore.sh <backup-root> [--apply]

Default: dry-run (no destructive changes).
Destructive apply requires:
  MARKHAND_RESTORE_CONFIRM=I_UNDERSTAND_DESTRUCTIVE_RESTORE
  restore.sh <backup-root> --apply
USAGE
  exit 2
fi

BACKUP_DIR="$(cd "$BACKUP_DIR" && pwd -P)"
DRY_RUN=1
if [[ "$APPLY" == "--apply" ]]; then
  DRY_RUN=0
  markhand_require_destructive_confirm
fi

MANIFEST="$BACKUP_DIR/recovery-manifest.json"
[[ -f "$MANIFEST" ]] || markhand_die "missing recovery-manifest.json"
"$SCRIPT_DIR/validate-manifest.sh" "$MANIFEST" "$BACKUP_DIR"

REPORT_DIR="${MARKHAND_RESTORE_REPORT_DIR:-$BACKUP_DIR/restore-report}"
mkdir -p "$REPORT_DIR"
export MARKHAND_RESTORE_REPORT_DIR="$REPORT_DIR"

STAGE="$(markhand_checkpoint_get "$BACKUP_DIR")"
markhand_log "restore start dry_run=$DRY_RUN resume_stage=$STAGE"

# Fence writes for restore window.
"$SCRIPT_DIR/fence-writes.sh" "$BACKUP_DIR/restore-fence.json"

# Always block readiness before mutating stores (even in dry-run we record SQL).
"$SCRIPT_DIR/reconcile-before-ready.sh" detect 1 >/dev/null || true
markhand_runtime_readiness_sql open "restore in progress" >"$REPORT_DIR/readiness-open.sql"

run_stage() {
  local name="$1"
  shift
  markhand_log "stage=$name"
  "$@"
  printf '%s\n' "$name" >>"$REPORT_DIR/stages.log"
}

# Resume support: skip completed destructive stages when checkpoint advanced.
need_pg=1
need_minio=1
need_qdrant=1
case "$STAGE" in
  postgres-restored) need_pg=0 ;;
  minio-restored) need_pg=0; need_minio=0 ;;
  qdrant-restored|vectors-rebuilt) need_pg=0; need_minio=0; need_qdrant=0 ;;
esac

if [[ "$need_pg" == "1" ]]; then
  run_stage "restore-postgres" "$SCRIPT_DIR/restore-postgres.sh" "$BACKUP_DIR" "$DRY_RUN"
fi
if [[ "$need_minio" == "1" ]]; then
  run_stage "restore-minio" "$SCRIPT_DIR/restore-minio.sh" "$BACKUP_DIR" "$DRY_RUN"
fi

QDRANT_OK=1
if [[ "$need_qdrant" == "1" ]]; then
  markhand_log "stage=restore-qdrant"
  if "$SCRIPT_DIR/restore-qdrant.sh" "$BACKUP_DIR" "$DRY_RUN"; then
    printf '%s\n' "restore-qdrant" >>"$REPORT_DIR/stages.log"
  else
    QDRANT_OK=0
    markhand_log "Qdrant snapshot restore failed — will use PG-only vector rebuild"
  fi
fi

if [[ "$QDRANT_OK" != "1" ]]; then
  markhand_log "Qdrant snapshot restore unavailable — PG-only vector rebuild path"
  run_stage "rebuild-vectors-from-pg" "$SCRIPT_DIR/rebuild-vectors-from-pg.sh" "$DRY_RUN"
  if [[ "$DRY_RUN" != "1" ]]; then
    markhand_checkpoint_set "$BACKUP_DIR" "vectors-rebuilt"
  fi
fi

# Reconcile before readiness. Dry-run keeps ready=false.
if [[ "$DRY_RUN" == "1" ]]; then
  run_stage "reconcile-detect-dry-run" "$SCRIPT_DIR/reconcile-before-ready.sh" detect 1
  cat >"$REPORT_DIR/summary.json" <<EOF
{
  "dryRun": true,
  "ready": false,
  "claimsLiveRestore": false,
  "claimsRpoRtoPass": false,
  "order": ["fence", "postgres", "minio", "qdrant_or_rebuild", "reconcile", "try_ready"]
}
EOF
  markhand_log "DRY-RUN restore complete — readiness remains false; no RPO/RTO claim"
  cat "$REPORT_DIR/summary.json"
  exit 0
fi

run_stage "reconcile-repair" "$SCRIPT_DIR/reconcile-before-ready.sh" repair 0

cat >"$REPORT_DIR/summary.json" <<EOF
{
  "dryRun": false,
  "ready": "see runtime_readiness",
  "claimsLiveRestore": true,
  "claimsRpoRtoPass": false,
  "note": "Profile-B RPO/RTO evidence remains a separate gate; this run only restores and reconciles."
}
EOF
markhand_log "restore apply finished — verify /api/v1/health/ready and reconcile reports"
cat "$REPORT_DIR/summary.json"
