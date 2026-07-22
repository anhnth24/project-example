#!/usr/bin/env bash
# Shared helpers for Markhand backup/restore (P1B-O03).
# shellcheck shell=bash
set -euo pipefail

markhand_backup_init() {
  local here
  here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  BACKUP_LIB_DIR="$here"
  BACKUP_ROOT_DIR="$(cd "$here/.." && pwd)"
  REPO_ROOT="$(cd "$here/../../.." && pwd)"
  export REPO_ROOT BACKUP_ROOT_DIR BACKUP_LIB_DIR

  # Optional PATH injection for hermetic fake CLIs (tests) or pinned tool dirs.
  if [[ -n "${MARKHAND_BACKUP_BIN_DIR:-}" ]]; then
    if [[ ! -d "$MARKHAND_BACKUP_BIN_DIR" ]]; then
      markhand_die "MARKHAND_BACKUP_BIN_DIR is not a directory: $MARKHAND_BACKUP_BIN_DIR"
    fi
    # Reject path traversal / symlink escapes outside the provided dir listing later.
    PATH="$MARKHAND_BACKUP_BIN_DIR:$PATH"
    export PATH
  fi

  MARKHAND_BACKUP_MODE="${MARKHAND_BACKUP_MODE:-live}"
  case "$MARKHAND_BACKUP_MODE" in
    live|hermetic|dry-run) ;;
    *) markhand_die "MARKHAND_BACKUP_MODE must be live|hermetic|dry-run" ;;
  esac

  MANIFEST_PY="$BACKUP_LIB_DIR/manifest.py"
  if [[ ! -f "$MANIFEST_PY" ]]; then
    markhand_die "missing manifest helper: $MANIFEST_PY"
  fi
}

markhand_die() {
  echo "error: $*" >&2
  exit 2
}

markhand_log() {
  # Never print values of secret-bearing env names.
  echo "[backup] $*" >&2
}

markhand_require_cmd() {
  local name="$1"
  if ! command -v "$name" >/dev/null 2>&1; then
    markhand_die "required command not found (fail closed): $name"
  fi
}

markhand_require_env() {
  local name="$1"
  if [[ -z "${!name:-}" ]]; then
    markhand_die "required env missing (fail closed): $name"
  fi
}

markhand_redact_line() {
  # Best-effort redaction for operator logs (IDs/paths ok; secrets not).
  sed -E \
    -e 's/(password|secret|api[_-]?key|token|authorization)=[^[:space:]]+/\1=***REDACTED***/Ig' \
    -e 's#postgres(ql)?://[^[:space:]]+#postgres://***REDACTED***#Ig' \
    -e 's/(MARKHAND_[A-Z0-9_]*(PASSWORD|SECRET|KEY|TOKEN))=[^[:space:]]+/\1=***REDACTED***/g'
}

markhand_safe_relpath() {
  local rel="$1"
  if [[ "$rel" == /* || "$rel" == *..* ]]; then
    markhand_die "unsafe path rejected: $rel"
  fi
  if [[ "$rel" =~ [[:space:]] ]]; then
    markhand_die "path with whitespace rejected: $rel"
  fi
}

markhand_resolve_under() {
  # Resolve path under root; reject traversal and symlinks.
  local root="$1"
  local rel="$2"
  markhand_safe_relpath "$rel"
  local root_abs candidate
  root_abs="$(cd "$root" && pwd -P)"
  candidate="$root_abs/$rel"
  if [[ -L "$candidate" ]]; then
    markhand_die "symlink rejected: $rel"
  fi
  if [[ ! -e "$candidate" ]]; then
    # Parent must exist and stay under root when creating.
    local parent
    parent="$(dirname "$candidate")"
    mkdir -p "$parent"
    parent="$(cd "$parent" && pwd -P)"
    case "$parent" in
      "$root_abs"|"$root_abs"/*) ;;
      *) markhand_die "path escapes backup root: $rel" ;;
    esac
    printf '%s\n' "$candidate"
    return 0
  fi
  candidate="$(cd "$(dirname "$candidate")" && pwd -P)/$(basename "$candidate")"
  case "$candidate" in
    "$root_abs"|"$root_abs"/*) ;;
    *) markhand_die "path escapes backup root: $rel" ;;
  esac
  printf '%s\n' "$candidate"
}

markhand_sha256_file() {
  local path="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$path" | awk '{print $1}'
  else
    openssl dgst -sha256 "$path" | awk '{print $NF}'
  fi
}

markhand_state_dir() {
  local root="$1"
  mkdir -p "$root/.state"
  printf '%s\n' "$root/.state"
}

markhand_checkpoint_set() {
  local root="$1"
  local stage="$2"
  local state
  state="$(markhand_state_dir "$root")"
  printf '%s\n' "$stage" >"$state/stage"
  printf '%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >"$state/updated_at"
}

markhand_checkpoint_get() {
  local root="$1"
  local state="$root/.state/stage"
  if [[ -f "$state" ]]; then
    cat "$state"
  else
    printf '%s\n' "none"
  fi
}

markhand_require_destructive_confirm() {
  local expected="I_UNDERSTAND_DESTRUCTIVE_RESTORE"
  if [[ "${MARKHAND_RESTORE_CONFIRM:-}" != "$expected" ]]; then
    markhand_die \
      "destructive restore refused: set MARKHAND_RESTORE_CONFIRM=$expected (default is dry-run / fail-closed)"
  fi
}

markhand_load_poc_env() {
  # Least-privilege: only source deploy/.env when present; never echo secrets.
  local env_file="${MARKHAND_ENV_FILE:-$REPO_ROOT/deploy/.env}"
  if [[ -f "$env_file" ]]; then
    set -a
    # shellcheck disable=SC1090
    source "$env_file"
    set +a
  fi
}

markhand_runtime_readiness_sql() {
  # Emit SQL that reuses 0022 helpers — readiness false until reconcile certifies.
  local action="$1"
  local detail="${2:-restore fence}"
  case "$action" in
    open)
      cat <<SQL
SELECT markhand_runtime_readiness_open('startup_reconciliation', '${detail//\'/\'\'}');
SQL
      ;;
    try_ready)
      cat <<SQL
SELECT markhand_runtime_readiness_try_ready('startup_reconciliation', '${detail//\'/\'\'}');
SQL
      ;;
    status)
      cat <<SQL
SELECT key, ready, generation, certified_generation, detail
FROM runtime_readiness
WHERE key = 'startup_reconciliation';
SQL
      ;;
    *)
      markhand_die "unknown readiness action: $action"
      ;;
  esac
}

markhand_psql() {
  markhand_require_cmd psql
  markhand_require_env MARKHAND_BACKUP_DATABASE_URL
  # URL carries credentials; never print it.
  psql "$MARKHAND_BACKUP_DATABASE_URL" -v ON_ERROR_STOP=1 "$@"
}

markhand_docker_available() {
  if ! command -v docker >/dev/null 2>&1; then
    return 1
  fi
  docker info >/dev/null 2>&1
}
