#!/usr/bin/env bash
# Shared compose argv helper for P1B-F02 POC scripts.
# shellcheck shell=bash

poc_compose_init() {
  ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
  COMPOSE_FILE="$ROOT/deploy/compose.poc.yml"
  ENV_FILE="$ROOT/deploy/.env"
  POC_COMPOSE_EFFECTIVE="${POC_COMPOSE_EFFECTIVE:-}"

  if [[ ! -f "$ENV_FILE" ]]; then
    cp "$ROOT/deploy/.env.example" "$ENV_FILE"
    echo "created $ENV_FILE from .env.example"
  fi

  set -a
  # shellcheck disable=SC1090
  source "$ENV_FILE"
  set +a

  export COMPOSE_PROFILES="${COMPOSE_PROFILES:-mock}"
  export DOCKER_BUILDKIT="${DOCKER_BUILDKIT:-0}"
  export COMPOSE_DOCKER_CLI_BUILD="${COMPOSE_DOCKER_CLI_BUILD:-0}"
  export COMPOSE_BAKE="${COMPOSE_BAKE:-false}"

  local files=("$COMPOSE_FILE")
  if poc_cgroup_limits_broken; then
    POC_COMPOSE_EFFECTIVE="$(poc_write_nolimit_compose)"
    files=("$POC_COMPOSE_EFFECTIVE")
    echo "NOTE: cgroup v2 cannot apply domain memory/cpu/pids limits here; using stripped compose for boot" >&2
  elif [[ -n "${POC_FORCE_NOLIMIT_COMPOSE:-}" ]]; then
    POC_COMPOSE_EFFECTIVE="$(poc_write_nolimit_compose)"
    files=("$POC_COMPOSE_EFFECTIVE")
  fi

  COMPOSE=(docker compose --env-file "$ENV_FILE")
  local f
  for f in "${files[@]}"; do
    COMPOSE+=(-f "$f")
  done
}

poc_cgroup_limits_broken() {
  # Nested hosts sometimes leave Docker's cgroup in threaded mode so runc cannot
  # apply domain controllers required by mem_limit/cpus/pids_limit.
  local ctype
  ctype="$(cat /sys/fs/cgroup/docker/cgroup.type 2>/dev/null || true)"
  [[ "$ctype" == "threaded" || "$ctype" == "invalid" ]]
}

poc_write_nolimit_compose() {
  local out="${TMPDIR:-/tmp}/markhand-poc-nolimit.$$.$RANDOM.yml"
  docker compose --env-file "$ENV_FILE" -f "$COMPOSE_FILE" config \
    | python3 -c '
import re, sys
text = sys.stdin.read()
out = []
for line in text.splitlines(True):
    if re.match(r"^\s+(mem_limit|cpus|pids_limit):\s*", line):
        continue
    out.append(line)
sys.stdout.write("".join(out))
' >"$out"
  echo "$out"
}
