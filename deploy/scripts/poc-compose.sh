#!/usr/bin/env bash
# Shared compose argv helper for P1B-F02 POC scripts.
# shellcheck shell=bash

_POC_EXIT_TRAP_CMDS=()
_POC_EXIT_TRAP_INSTALLED=0
POC_PYTHON_BIN="${POC_PYTHON_BIN:-python3}"
if ! "$POC_PYTHON_BIN" -c 'pass' >/dev/null 2>&1 && command -v python >/dev/null 2>&1; then
  POC_PYTHON_BIN=python
fi

_poc_run_exit_traps() {
  local status="${1:-$?}"
  local cmd
  trap - EXIT
  for cmd in "${_POC_EXIT_TRAP_CMDS[@]}"; do
    eval "$cmd" || true
  done
  return "$status"
}

_poc_add_exit_trap() {
  local cmd="$1"
  local existing
  if [[ "$_POC_EXIT_TRAP_INSTALLED" -eq 0 ]]; then
    existing="$(trap -p EXIT || true)"
    if [[ "$existing" =~ ^trap\ --\ \'(.*)\'\ EXIT$ ]]; then
      _POC_EXIT_TRAP_CMDS+=("${BASH_REMATCH[1]}")
    fi
    trap '_poc_run_exit_traps $?' EXIT
    _POC_EXIT_TRAP_INSTALLED=1
  fi
  _POC_EXIT_TRAP_CMDS+=("$cmd")
}

poc_compose_init() {
  ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
  COMPOSE_FILE="$ROOT/deploy/compose.poc.yml"
  ENV_FILE="$ROOT/deploy/.env"
  POC_COMPOSE_EFFECTIVE="${POC_COMPOSE_EFFECTIVE:-}"
  local requested_project="${MARKHAND_COMPOSE_PROJECT:-}"

  if [[ ! -f "$ENV_FILE" ]]; then
    cp "$ROOT/deploy/.env.example" "$ENV_FILE"
    echo "created $ENV_FILE from .env.example"
  fi

  set -a
  # shellcheck disable=SC1090
  source "$ENV_FILE"
  set +a
  if [[ -n "$requested_project" ]]; then
    export MARKHAND_COMPOSE_PROJECT="$requested_project"
  fi

  poc_resolve_convert_apparmor

  export COMPOSE_PROFILES="${COMPOSE_PROFILES:-mock}"
  export DOCKER_BUILDKIT="${DOCKER_BUILDKIT:-0}"
  export COMPOSE_DOCKER_CLI_BUILD="${COMPOSE_DOCKER_CLI_BUILD:-0}"
  export COMPOSE_BAKE="${COMPOSE_BAKE:-false}"

  local files=("$COMPOSE_FILE")
  if poc_cgroup_limits_broken; then
    poc_write_nolimit_compose >/dev/null
    files=("$POC_COMPOSE_EFFECTIVE")
    echo "NOTE: cgroup v2 cannot apply domain memory/cpu/pids limits here; using stripped compose for boot" >&2
  elif [[ -n "${POC_FORCE_NOLIMIT_COMPOSE:-}" ]]; then
    poc_write_nolimit_compose >/dev/null
    files=("$POC_COMPOSE_EFFECTIVE")
  fi

  COMPOSE=(docker compose --env-file "$ENV_FILE")
  local f
  for f in "${files[@]}"; do
    COMPOSE+=(-f "$f")
  done
}

_poc_cleanup_nolimit_compose() {
  if [[ -n "${POC_COMPOSE_EFFECTIVE:-}" && -f "$POC_COMPOSE_EFFECTIVE" ]]; then
    rm -f -- "$POC_COMPOSE_EFFECTIVE"
  fi
}

poc_resolve_convert_apparmor() {
  # The convert worker needs an AppArmor profile that permits mount, because its
  # nested sandbox remounts / as MS_REC|MS_PRIVATE. Hosts without AppArmor have
  # nothing to enforce, so they run unconfined; hosts with AppArmor must have the
  # profile loaded or the container would start under a policy that breaks the
  # sandbox in a way that only shows up as a crash loop.
  if [[ -n "${MARKHAND_CONVERTER_APPARMOR_PROFILE:-}" ]]; then
    export MARKHAND_CONVERTER_APPARMOR_PROFILE
    return 0
  fi
  if [[ ! -d /sys/kernel/security/apparmor ]]; then
    export MARKHAND_CONVERTER_APPARMOR_PROFILE=unconfined
    return 0
  fi
  local profiles=/sys/kernel/security/apparmor/profiles
  if [[ -r "$profiles" ]] && ! grep -q '^markhand-convert[ (]' "$profiles"; then
    echo "AppArmor is enabled but the markhand-convert profile is not loaded." >&2
    echo "Load it once:  sudo apparmor_parser -r -W $ROOT/deploy/poc/apparmor-markhand-convert" >&2
    return 1
  fi
  # The profile list is root-only on most distributions; when it cannot be read,
  # let Docker report a missing profile instead of guessing.
  export MARKHAND_CONVERTER_APPARMOR_PROFILE=markhand-convert
}

poc_cgroup_limits_broken() {
  # Nested hosts sometimes leave Docker's cgroup in threaded mode so runc cannot
  # apply domain controllers required by mem_limit/cpus/pids_limit.
  local ctype
  ctype="$(cat /sys/fs/cgroup/docker/cgroup.type 2>/dev/null || true)"
  [[ "$ctype" == "threaded" || "$ctype" == "invalid" ]]
}

poc_write_nolimit_compose() {
  local out old_umask
  ROOT="${ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
  old_umask="$(umask)"
  umask 077
  out="$(mktemp "${TMPDIR:-/tmp}/markhand-poc-nolimit.XXXXXX.yml")"
  umask "$old_umask"
  POC_COMPOSE_EFFECTIVE="$out"
  _poc_add_exit_trap '_poc_cleanup_nolimit_compose'
  docker compose --env-file "$ENV_FILE" -f "$COMPOSE_FILE" config \
    | "$POC_PYTHON_BIN" -c '
import re, sys
text = sys.stdin.read()
out = []
for line in text.splitlines(True):
    if re.match(r"^\s+(mem_limit|cpus|pids_limit):\s*", line):
        continue
    out.append(line)
sys.stdout.write("".join(out))
' >"$out"
  chmod 600 "$out" 2>/dev/null || true
  if [[ "$out" == "$ROOT"/* ]]; then
    echo "refusing nolimit effective compose inside repository: $out" >&2
    return 1
  fi
  echo "$out"
}
