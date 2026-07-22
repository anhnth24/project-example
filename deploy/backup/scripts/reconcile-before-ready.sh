#!/usr/bin/env bash
# Keep readiness blocked, run reconcile detect→repair via runtime helpers / worker,
# and only attempt markhand_runtime_readiness_try_ready after verified convergence.
#
# Reuses migration 0022 helpers:
#   markhand_runtime_readiness_open / markhand_runtime_readiness_try_ready
# and the reconcile worker (MARKHAND_WORKER_KIND=reconcile). There is no ad-hoc
# admin SQL bypass of the readiness fence.
# shellcheck shell=bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../lib/common.sh
source "$SCRIPT_DIR/../lib/common.sh"
markhand_backup_init
markhand_load_poc_env

MODE="${1:-detect}"
DRY_RUN="${2:-1}"
case "$MODE" in
  detect|repair) ;;
  *) markhand_die "usage: reconcile-before-ready.sh <detect|repair> [dry_run=1]" ;;
esac

markhand_require_env MARKHAND_BACKUP_DATABASE_URL

REPORT_DIR="${MARKHAND_RESTORE_REPORT_DIR:-/tmp/markhand-restore-reconcile}"
mkdir -p "$REPORT_DIR"
STATUS_OUT="$REPORT_DIR/readiness-status.txt"
DETECT_OUT="$REPORT_DIR/reconcile-detect.json"
REPAIR_OUT="$REPORT_DIR/reconcile-repair.json"

# Always open/block readiness first (fail closed).
markhand_log "opening runtime_readiness fence (ready=false)"
if [[ "$MARKHAND_BACKUP_MODE" == "hermetic" ]]; then
  printf 'ready=false\n' >"$STATUS_OUT"
  markhand_runtime_readiness_sql open "post-restore reconcile fence" >"$REPORT_DIR/open.sql"
else
  markhand_psql -c "$(markhand_runtime_readiness_sql open "post-restore reconcile fence")" \
    | markhand_redact_line | tee "$STATUS_OUT" >/dev/null
fi

run_worker_once() {
  local worker_mode="$1"
  local out="$2"
  # Documented operator path: start reconcile worker against restored stack.
  # Hermetic fixtures use MARKHAND_FAKE_RECONCILE_RESULT.
  if [[ "$MARKHAND_BACKUP_MODE" == "hermetic" ]]; then
    local result="${MARKHAND_FAKE_RECONCILE_RESULT:-ok}"
    python3 - "$out" "$worker_mode" "$result" <<'PY'
import json, sys
out, mode, result = sys.argv[1:]
payload = {
    "mode": mode,
    "result": result,
    "missing_objects": 0 if result == "ok" else 1,
    "orphan_objects": 0,
    "missing_vectors": 0 if result == "ok" else 1,
    "orphan_vectors": 0,
    "stale_vectors": 0,
    "converged": result == "ok",
}
with open(out, "w", encoding="utf-8") as handle:
    json.dump(payload, handle, indent=2)
    handle.write("\n")
PY
    return 0
  fi

  markhand_require_env MARKHAND_WORKER_ORG_ID
  if markhand_docker_available && [[ -f "$REPO_ROOT/deploy/scripts/poc-compose.sh" ]]; then
    # shellcheck source=../../scripts/poc-compose.sh
    source "$REPO_ROOT/deploy/scripts/poc-compose.sh"
    poc_compose_init
    "${COMPOSE[@]}" up -d worker-reconcile
    # Poll readiness SQL — do not invent unsupported CLI flags.
    local i
    for i in $(seq 1 60); do
      markhand_psql -At -c "SELECT ready FROM runtime_readiness WHERE key='startup_reconciliation';" \
        >"$STATUS_OUT" || true
      sleep 2
    done
    printf '{"mode":"%s","note":"worker-reconcile started; see runtime_readiness"}\n' "$worker_mode" >"$out"
    return 0
  fi
  markhand_die "cannot run live reconcile without Docker compose or hermetic mode"
}

if [[ "$DRY_RUN" == "1" ]]; then
  markhand_log "DRY-RUN reconcile mode=$MODE — readiness remains blocked; no try_ready"
  printf '{"mode":"%s","dryRun":true,"ready":false}\n' "$MODE" >"$DETECT_OUT"
  cat "$DETECT_OUT"
  exit 0
fi

markhand_require_destructive_confirm

run_worker_once "detect" "$DETECT_OUT"
CONVERGED="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("converged", False))' "$DETECT_OUT")"

if [[ "$MODE" == "repair" ]]; then
  run_worker_once "repair" "$REPAIR_OUT"
  CONVERGED="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("converged", False))' "$REPAIR_OUT")"
fi

if [[ "$CONVERGED" != "True" && "$CONVERGED" != "true" ]]; then
  markhand_log "reconcile not converged — readiness stays false"
  if [[ "$MARKHAND_BACKUP_MODE" != "hermetic" ]]; then
    markhand_psql -c "$(markhand_runtime_readiness_sql status)" | markhand_redact_line || true
  fi
  exit 1
fi

# Only after verified convergence attempt certify ready.
if [[ "$MARKHAND_BACKUP_MODE" == "hermetic" ]]; then
  printf 'ready=true\n' >"$STATUS_OUT"
  markhand_runtime_readiness_sql try_ready "post-restore reconcile certified" >"$REPORT_DIR/try_ready.sql"
else
  markhand_psql -c "$(markhand_runtime_readiness_sql try_ready "post-restore reconcile certified")" \
    | markhand_redact_line | tee "$STATUS_OUT" >/dev/null
fi

markhand_log "reconcile converged; try_ready issued"
cat "$STATUS_OUT"
