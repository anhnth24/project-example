#!/usr/bin/env bash
# P1B-O02 postgres restore-arm protocol (sourced or standalone failpoint harness).
#
# Semantics:
#   - Arm PG_RESTORE_ARMED=1 *before* docker stop.
#   - EXIT/restore only acts when armed.
#   - Disarm only after a *confirmed* restart (State.Running=true).
#   - Initially stopped / never-armed: EXIT must not start the container.
#
# Failpoints when executed directly:
#   before_stop | during_stop | after_stop | initially_stopped | normal

PG_RESTORE_ARMED="${PG_RESTORE_ARMED:-0}"
PG_CID="${PG_CID:-}"

o02_pg_arm_restore() {
  PG_RESTORE_ARMED=1
}

o02_pg_disarm_restore_if_running() {
  local cid="${1:-$PG_CID}"
  [[ -n "$cid" ]] || return 1
  if docker inspect -f '{{.State.Running}}' "$cid" 2>/dev/null | grep -qi true; then
    PG_RESTORE_ARMED=0
    return 0
  fi
  return 1
}

o02_pg_restore_if_armed() {
  local cid="${1:-$PG_CID}"
  if [[ "${PG_RESTORE_ARMED}" -ne 1 || -z "$cid" ]]; then
    echo "restore_skipped armed=${PG_RESTORE_ARMED:-0}"
    return 0
  fi
  docker start "$cid" >/dev/null 2>&1 || true
  if o02_pg_disarm_restore_if_running "$cid"; then
    echo "restored=1 disarmed=1"
  else
    echo "restored_attempted=1 disarmed=0"
  fi
}

# Standalone failpoint runner when executed directly.
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  set -euo pipefail
  MODE="${1:?mode required}"
  CID="${2:?container id required}"
  OUT="${3:?output file required}"
  PG_CID="$CID"
  PG_RESTORE_ARMED=0
  : >"$OUT"
  trap 'o02_pg_restore_if_armed "$CID" | tee -a "$OUT" >/dev/null' EXIT

  case "$MODE" in
    before_stop)
      if ! docker inspect -f '{{.State.Running}}' "$CID" | grep -qi true; then
        echo "precondition_not_running=1" >>"$OUT"
        exit 0
      fi
      o02_pg_arm_restore
      echo "armed=1 before_stop=1" >>"$OUT"
      # Failpoint: exit before stop — restore may start an already-running container (ok).
      exit 0
      ;;
    during_stop)
      if ! docker inspect -f '{{.State.Running}}' "$CID" | grep -qi true; then
        echo "precondition_not_running=1" >>"$OUT"
        exit 0
      fi
      o02_pg_arm_restore
      echo "armed=1" >>"$OUT"
      docker stop "$CID" >/dev/null &
      stop_pid=$!
      sleep 0.2
      echo "during_stop=1" >>"$OUT"
      kill -TERM $$
      wait "$stop_pid" || true
      sleep 2
      ;;
    after_stop)
      if ! docker inspect -f '{{.State.Running}}' "$CID" | grep -qi true; then
        echo "precondition_not_running=1" >>"$OUT"
        exit 0
      fi
      o02_pg_arm_restore
      echo "armed=1" >>"$OUT"
      docker stop "$CID" >/dev/null
      echo "stopped=1" >>"$OUT"
      exit 0
      ;;
    initially_stopped)
      echo "inject=0 armed=0" >>"$OUT"
      exit 0
      ;;
    normal)
      if ! docker inspect -f '{{.State.Running}}' "$CID" | grep -qi true; then
        echo "precondition_not_running=1" >>"$OUT"
        exit 0
      fi
      o02_pg_arm_restore
      echo "armed=1" >>"$OUT"
      docker stop "$CID" >/dev/null
      echo "stopped=1" >>"$OUT"
      docker start "$CID" >/dev/null
      if o02_pg_disarm_restore_if_running "$CID"; then
        echo "confirmed_restart=1 disarmed=1" >>"$OUT"
      else
        echo "confirmed_restart=0" >>"$OUT"
      fi
      ;;
    *)
      echo "bad_mode=$MODE" >>"$OUT"
      exit 2
      ;;
  esac
fi
