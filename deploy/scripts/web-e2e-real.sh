#!/usr/bin/env bash
# Real-deployment half of P2-15/P2-20: build the web SPA, boot fileconv-server plus
# convert/index/embedding/delete workers against the already-up dev Compose stack
# (Postgres/Qdrant/MinIO/embedding), set up a run-scoped fixture, and run the
# Playwright `real` project against it. Fail-closed on fixture/setup/cleanup,
# process death, redactor failure, artifact validation, or secret/content canary.
#
# Pattern follows server-smoke.sh (init env -> source .env -> bootstrap ->
# migrate -> qdrant-init -> run fileconv-server + workers -> poll /health/ready
# -> seed) but does not tear processes down until Playwright has run, and
# additionally builds + serves the SPA from the same process (see
# deploy/README.md's "Web SPA static serving (P2-16)" section).
#
# Dev-only lowered knobs (this process only; not production defaults):
#   MARKHAND_MAX_UPLOAD_BYTES=4096          — deterministic real 413
#   MARKHAND_RATE_ROUTE_PER_MINUTE=1       — deterministic real reindex 429
# These already exist as ops overrides in config.rs / rate_limit.rs.
#
# Prerequisites this script assumes the caller already set up (mirrors
# server-smoke.sh's own assumptions about a ready Rust toolchain):
#   - the dev Compose stack is up (`make dev-up` / dev-stack-ci.sh)
#   - Node/pnpm are on PATH and `pnpm install --frozen-lockfile` has run
#   - Playwright's Chromium is installed (`pnpm --dir web exec playwright
#     install --with-deps chromium`)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
ORCHESTRATION_TEST="${WEB_E2E_REAL_ORCHESTRATION_TEST:-0}"
ENV_FILE="${WEB_E2E_REAL_ENV_FILE:-$ROOT/deploy/dev/.env}"
WEB_DIR="$ROOT/web"
REDACT="$ROOT/deploy/scripts/redact_secrets.py"
FIXTURE_CLI="$ROOT/deploy/scripts/web_e2e_real_fixture.py"
ARTIFACTS_CLI="$ROOT/deploy/scripts/web_e2e_real_artifacts.py"
SERVER_BIN="${WEB_E2E_REAL_SERVER_BIN:-$ROOT/target/debug/fileconv-server}"
WORKER_BIN="${WEB_E2E_REAL_WORKER_BIN:-$ROOT/target/debug/fileconv-worker}"
FILECONV_BIN="${WEB_E2E_REAL_FILECONV_BIN:-$ROOT/target/debug/fileconv}"

# Mock signature / dims must match deploy/dev/.env.example mock block and mock-embedding.py.
MOCK_INDEX_SIGNATURE="0f59a26d542340c3c2c062a227417e47f9303c2db67569cf9031fe4707e44bf0"

if [[ -z "${WEB_E2E_REAL_RUN_ID:-}" ]]; then
  WEB_E2E_REAL_RUN_ID="$(python3 -c 'import uuid; print("e2e-" + str(uuid.uuid4()))')"
fi
export WEB_E2E_REAL_RUN_ID
export MARKHAND_E2E_REAL_RUN_ID="$WEB_E2E_REAL_RUN_ID"

WEB_E2E_REAL_ARTIFACT_DIR="${WEB_E2E_REAL_ARTIFACT_DIR:-/tmp/web-e2e-real-${WEB_E2E_REAL_RUN_ID}}"
export WEB_E2E_REAL_ARTIFACT_DIR
mkdir -p "$WEB_E2E_REAL_ARTIFACT_DIR"

# Public fixture manifest is staged; credentials stay outside the artifact tree.
FIXTURE_MANIFEST="${WEB_E2E_REAL_FIXTURE_MANIFEST:-$WEB_E2E_REAL_ARTIFACT_DIR/fixture-manifest.json}"
CREDENTIALS_FILE="${WEB_E2E_REAL_CREDENTIALS_FILE:-${WEB_E2E_REAL_ARTIFACT_DIR}.credentials.json}"
PLAYWRIGHT_RESULTS="${WEB_E2E_REAL_PLAYWRIGHT_RESULTS:-$WEB_E2E_REAL_ARTIFACT_DIR/playwright-results.json}"
SANITIZED_MANIFEST="${WEB_E2E_REAL_SANITIZED_MANIFEST:-$WEB_E2E_REAL_ARTIFACT_DIR/manifest.json}"
export WEB_E2E_REAL_PLAYWRIGHT_RESULTS="$PLAYWRIGHT_RESULTS"

# Dev-only lowered knobs for deterministic 413/429 in this process only.
# Values are forced again after sourcing .env (see below).
export MARKHAND_MAX_UPLOAD_BYTES=4096
export MARKHAND_RATE_ROUTE_PER_MINUTE=1

server_pid=""
worker_pids=()
server_log=""
convert_log=""
index_log=""
embedding_log=""
delete_log=""
created_env=false
fixture_setup_ok=false
fixture_cleanup_status=0
playwright_status=0
artifact_status=0
job_status=0

cleanup() {
  local status=$?
  local worker_pid

  if [[ -n "${WEB_E2E_REAL_CLEANUP_DELAY_SECS:-}" ]]; then
    sleep "$WEB_E2E_REAL_CLEANUP_DELAY_SECS"
  fi

  # Fixture cleanup must run while server + delete worker are still alive.
  if [[ "$fixture_setup_ok" == true ]]; then
    set +e
    python3 "$FIXTURE_CLI" cleanup \
      --run-id "$WEB_E2E_REAL_RUN_ID" \
      --manifest "$FIXTURE_MANIFEST" \
      --credentials "$CREDENTIALS_FILE" \
      --api-base "http://${bind_addr:-127.0.0.1:8787}" \
      --timeout-secs "${WEB_E2E_REAL_FIXTURE_CLEANUP_TIMEOUT_SECS:-120}"
    fixture_cleanup_status=$?
    set -e
    if [[ "$fixture_cleanup_status" -ne 0 ]]; then
      echo "web-e2e-real: fixture cleanup failed (exit ${fixture_cleanup_status})" >&2
      if [[ "$status" -eq 0 ]]; then
        status="$fixture_cleanup_status"
      fi
    fi
    if [[ -f "$SANITIZED_MANIFEST" ]]; then
      local teardown_value="ok"
      if [[ "$fixture_cleanup_status" -ne 0 ]]; then
        teardown_value="failed"
      fi
      python3 - "$SANITIZED_MANIFEST" "$teardown_value" <<'PY' || true
import json
import sys
from pathlib import Path
path = Path(sys.argv[1])
payload = json.loads(path.read_text(encoding="utf-8"))
payload["teardownResult"] = sys.argv[2]
path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
PY
    fi
  fi

  if [[ -n "$server_pid" ]]; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  for worker_pid in "${worker_pids[@]}"; do
    kill "$worker_pid" 2>/dev/null || true
  done
  for worker_pid in "${worker_pids[@]}"; do
    wait "$worker_pid" 2>/dev/null || true
  done
  if [[ -n "$server_log" ]]; then
    rm -f "$server_log" "$convert_log" "$index_log" "$embedding_log" "$delete_log"
  fi
  if [[ "$created_env" == true ]]; then
    rm -f "$ENV_FILE"
  fi

  # Prefer the worst known failure so cleanup failure cannot greenwash Playwright.
  if [[ "$job_status" -ne 0 && "$status" -eq 0 ]]; then
    status="$job_status"
  fi
  if [[ "$fixture_cleanup_status" -ne 0 && "$status" -eq 0 ]]; then
    status="$fixture_cleanup_status"
  fi
  exit "$status"
}

server_log="$(mktemp)"
convert_log="$(mktemp)"
index_log="$(mktemp)"
embedding_log="$(mktemp)"
delete_log="$(mktemp)"
trap cleanup EXIT

dump_redacted_logs() {
  local reason="${1:-failure}"
  echo "=== web-e2e-real: ${reason} ===" >&2
  local entries=(
    "server:$server_log"
    "convert:$convert_log"
    "index:$index_log"
    "embedding:$embedding_log"
    "delete:$delete_log"
  )
  local entry label path
  for entry in "${entries[@]}"; do
    label="${entry%%:*}"
    path="${entry#*:}"
    echo "--- ${label} ---" >&2
    if [[ ! -f "$path" ]]; then
      echo "(no log file)" >&2
      continue
    fi
    if ! python3 "$REDACT" "$path" >&2; then
      echo "(log redaction failed for ${label} — raw log withheld)" >&2
    fi
  done
}

required_processes_alive() {
  local context="${1:-before Playwright}"
  if [[ -z "$server_pid" ]] || ! kill -0 "$server_pid" 2>/dev/null; then
    dump_redacted_logs "fileconv-server exited ${context}"
    return 1
  fi
  local worker_pid
  for worker_pid in "${worker_pids[@]}"; do
    if ! kill -0 "$worker_pid" 2>/dev/null; then
      dump_redacted_logs "worker process ${worker_pid} exited ${context}"
      return 1
    fi
  done
  return 0
}

start_worker() {
  local kind="$1"
  local worker_id="$2"
  local worker_log="$3"
  local converter_argv="${4:-}"

  (
    unset MARKHAND_AUTH_ISSUER MARKHAND_AUTH_AUDIENCE MARKHAND_AUTH_SIGNING_KEY MARKHAND_AUTH_KID
    unset MARKHAND_DATABASE_URL MARKHAND_MIGRATOR_DATABASE_URL

    export MARKHAND_WORKER_DATABASE_URL
    export MARKHAND_WORKER_ORG_ID="${MARKHAND_WORKER_ORG_ID:-11111111-1111-1111-1111-111111111111}"
    export MARKHAND_WORKER_USER_ID="${MARKHAND_WORKER_USER_ID:-22222222-2222-2222-2222-222222222201}"
    case "$kind" in
      convert) export MARKHAND_WORKER_KIND=convert ;;
      index) export MARKHAND_WORKER_KIND=index ;;
      embedding) export MARKHAND_WORKER_KIND=embedding ;;
      delete) export MARKHAND_WORKER_KIND=delete ;;
      *) echo "unknown worker kind: $kind" >&2; exit 1 ;;
    esac
    export MARKHAND_WORKER_ID="$worker_id"
    if [[ -n "$converter_argv" ]]; then
      export MARKHAND_CONVERTER_ARGV_JSON="$converter_argv"
    fi

    exec "$WORKER_BIN" >"$worker_log" 2>&1
  ) &
  worker_pids+=("$!")
}

run_playwright_supervised() {
  local playwright_pid playwright_status=0
  set +e
  if [[ -n "${WEB_E2E_REAL_PLAYWRIGHT_CMD:-}" ]]; then
    bash -c "$WEB_E2E_REAL_PLAYWRIGHT_CMD" &
    playwright_pid=$!
  else
    MARKHAND_E2E_REAL=1 \
      MARKHAND_E2E_REAL_BASE_URL="http://${bind_addr}" \
      MARKHAND_E2E_REAL_CREDENTIALS_FILE="$CREDENTIALS_FILE" \
      MARKHAND_E2E_REAL_FIXTURE_FILE="$FIXTURE_MANIFEST" \
      MARKHAND_E2E_REAL_RUN_ID="$WEB_E2E_REAL_RUN_ID" \
      WEB_E2E_REAL_PLAYWRIGHT_RESULTS="$PLAYWRIGHT_RESULTS" \
      pnpm --dir "$WEB_DIR" exec playwright test --project=real --reporter=json \
      >"$PLAYWRIGHT_RESULTS" &
    playwright_pid=$!
  fi

  while kill -0 "$playwright_pid" 2>/dev/null; do
    if ! required_processes_alive "during Playwright"; then
      kill -TERM "$playwright_pid" 2>/dev/null || true
      local _w
      for _w in $(seq 1 20); do
        kill -0 "$playwright_pid" 2>/dev/null || break
        sleep 0.05
      done
      kill -KILL "$playwright_pid" 2>/dev/null || true
      wait "$playwright_pid" 2>/dev/null || true
      return 1
    fi
    sleep 0.1
  done

  wait "$playwright_pid" 2>/dev/null || playwright_status=$?
  return "$playwright_status"
}

write_and_validate_artifacts() {
  local teardown_result="${1:-pending}"
  python3 "$ARTIFACTS_CLI" write \
    --results "$PLAYWRIGHT_RESULTS" \
    --fixture "$FIXTURE_MANIFEST" \
    --out "$SANITIZED_MANIFEST" \
    --teardown-result "$teardown_result"
  python3 "$ARTIFACTS_CLI" validate \
    --manifest "$SANITIZED_MANIFEST" \
    --artifact-dir "$WEB_E2E_REAL_ARTIFACT_DIR"
}

if [[ "$ORCHESTRATION_TEST" != "1" && ! -f "$ENV_FILE" ]]; then
  "$ROOT/deploy/scripts/init-dev-env.sh"
  created_env=true
fi

# CI sets COMPOSE_PROFILES=mock; do not let .env clobber it.
incoming_profiles="${COMPOSE_PROFILES:-}"

set -a
# shellcheck disable=SC1090
source "$ENV_FILE"
set +a

if [[ -n "$incoming_profiles" ]]; then
  export COMPOSE_PROFILES="$incoming_profiles"
fi

if [[ "${COMPOSE_PROFILES:-}" == *mock* ]]; then
  export MARKHAND_EMBEDDING_MODEL=markhand-mock
  export MARKHAND_EMBEDDING_REVISION=dev-local
  export MARKHAND_EMBEDDING_DIMENSIONS=8
  export MARKHAND_EMBEDDING_API_KEY=dev-mock-key
  export MARKHAND_INDEX_SIGNATURE="$MOCK_INDEX_SIGNATURE"
  export MARKHAND_MOCK_EMBEDDING_DIMENSIONS=8
fi

# Dev-only lowered knobs for deterministic 413/429 in this process only.
# Re-assert after sourcing .env so the harness keeps these ops overrides.
export MARKHAND_MAX_UPLOAD_BYTES=4096
export MARKHAND_RATE_ROUTE_PER_MINUTE=1

if [[ "$ORCHESTRATION_TEST" != "1" ]]; then
  "$ROOT/deploy/scripts/bootstrap-server-role.sh"
  "$ROOT/deploy/scripts/migrate.sh"
  python3 "$ROOT/deploy/poc/qdrant-init.py"
fi

# Build the SPA before the server starts: resolve_web_dist_dir() is checked
# once at router bootstrap, so a `web/dist` that appears after the server is
# already running would never be picked up.
pnpm --dir "$WEB_DIR" build
export MARKHAND_WEB_DIST_DIR="$WEB_DIR/dist"

export MARKHAND_WORKER_DATABASE_URL="postgres://${MARKHAND_WORKER_DB_USER:-markhand_worker}:${MARKHAND_WORKER_DB_PASSWORD:-markhand_worker_dev_only}@127.0.0.1:${MARKHAND_POSTGRES_PORT:-54329}/${MARKHAND_POSTGRES_DB:-markhand}"
export MARKHAND_CONVERTER_ARGV_JSON="[\"${FILECONV_BIN}\",\"one\",\"{input}\"]"

cargo build -p fileconv-server
cargo build -p fileconv-cli --bin fileconv

start_worker convert e2e-real-convert-1 "$convert_log" "$MARKHAND_CONVERTER_ARGV_JSON"
start_worker index e2e-real-index-1 "$index_log"
start_worker embedding e2e-real-embedding-1 "$embedding_log"
start_worker delete e2e-real-delete-1 "$delete_log"

"$SERVER_BIN" >"$server_log" 2>&1 &
server_pid=$!

bind_addr="${MARKHAND_BIND_ADDR:-127.0.0.1:8787}"
readiness_attempts="${WEB_E2E_REAL_READINESS_ATTEMPTS:-60}"
healthy=false
for _ in $(seq 1 "$readiness_attempts"); do
  if curl --fail --silent --show-error \
    "http://${bind_addr}/api/v1/health/ready" >/dev/null; then
    if kill -0 "$server_pid" 2>/dev/null; then
      healthy=true
      break
    fi
    dump_redacted_logs "fileconv-server exited during readiness"
    exit 1
  fi
  if ! kill -0 "$server_pid" 2>/dev/null; then
    dump_redacted_logs "fileconv-server exited during readiness"
    exit 1
  fi
  if ! required_processes_alive "during readiness"; then
    exit 1
  fi
  sleep 1
done

if [[ "$healthy" != true ]]; then
  dump_redacted_logs "fileconv-server never became ready"
  echo "unhealthy: fileconv-server" >&2
  exit 1
fi

if ! required_processes_alive "after readiness, before seed"; then
  exit 1
fi

"$ROOT/deploy/scripts/seed-dev-all.sh" --skip-init
echo "healthy: fileconv-server + convert/index/embedding/delete workers (web/dist from $MARKHAND_WEB_DIST_DIR)"

if ! required_processes_alive "after seed, before Playwright"; then
  exit 1
fi

# Run-scoped fixture before Playwright; failure aborts without starting the browser suite.
if ! python3 "$FIXTURE_CLI" setup \
  --run-id "$WEB_E2E_REAL_RUN_ID" \
  --manifest-out "$FIXTURE_MANIFEST" \
  --credentials-out "$CREDENTIALS_FILE"; then
  echo "web-e2e-real: fixture setup failed" >&2
  job_status=1
  exit 1
fi
fixture_setup_ok=true

export MARKHAND_E2E_REAL_CREDENTIALS_FILE="$CREDENTIALS_FILE"
export MARKHAND_E2E_REAL_FIXTURE_FILE="$FIXTURE_MANIFEST"
export MARKHAND_E2E_REAL_RUN_ID="$WEB_E2E_REAL_RUN_ID"

playwright_status=0
run_playwright_supervised || playwright_status=$?
if [[ "$playwright_status" -ne 0 ]]; then
  dump_redacted_logs "Playwright real project failed (exit ${playwright_status})"
  job_status="$playwright_status"
fi

# Artifact write/validate runs after Playwright even on failure when results exist.
set +e
if [[ -f "$PLAYWRIGHT_RESULTS" ]]; then
  write_and_validate_artifacts "pending"
  artifact_status=$?
else
  echo "web-e2e-real: missing playwright results for artifact validation" >&2
  artifact_status=1
fi
set -e
if [[ "$artifact_status" -ne 0 ]]; then
  echo "web-e2e-real: artifact validation failed (exit ${artifact_status})" >&2
  if [[ "$job_status" -eq 0 ]]; then
    job_status="$artifact_status"
  fi
fi

if [[ "$job_status" -ne 0 ]]; then
  exit "$job_status"
fi

exit 0
