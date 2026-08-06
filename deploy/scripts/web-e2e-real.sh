#!/usr/bin/env bash
# Real-deployment half of P2-15/P2-20: build the web SPA, boot fileconv-server
# plus convert/index/embedding/delete workers against the already-up dev Compose
# stack (Postgres/Qdrant/MinIO/embedding), create a run-scoped fixture, and run
# the Playwright `real` project against it.
#
# Pattern follows server-smoke.sh (init env -> source .env -> bootstrap ->
# migrate -> qdrant-init -> run-scoped fixture -> run fileconv-server + workers
# -> poll /health/ready -> seed) but does not tear processes down until
# Playwright has run, and additionally builds + serves the SPA from the same
# process (see deploy/README.md's "Web SPA static serving (P2-16)" section).
#
# Dev-only process-local server knobs (not test seams / bypasses / schema forks):
#   MARKHAND_MAX_UPLOAD_BYTES=4096          → deterministic HTTP 413
#   MARKHAND_RATE_ROUTE_PER_MINUTE=1        → deterministic reindex/upload HTTP 429
# Login stays on MARKHAND_RATE_AUTH_PER_MINUTE only (not this route knob), so a
# lowered route capacity does not starve subsequent logins or fixture cleanup.
# These already exist in crates/server config + rate_limit middleware. Production
# profile validation continues to refuse unsafe defaults.
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

server_pid=""
worker_pids=()
server_log=""
convert_log=""
index_log=""
embedding_log=""
delete_log=""
created_env=false
created_artifact_dir=false
created_runtime_dir=false
fixture_ready=false
fixture_file=""
credentials_file=""
playwright_results=""
artifact_dir=""
runtime_dir=""
run_id=""
bind_addr=""
job_status=0
cleanup_status=0
cleanup_started=false
redactor_failed=false
artifact_failed=false
teardown_result="pending"

upgrade_status() {
  local candidate="$1"
  if [[ "$candidate" -eq 0 ]]; then
    return 0
  fi
  if [[ "$job_status" -eq 0 ]]; then
    job_status="$candidate"
  fi
}

safe_rm_tree() {
  local path="$1"
  local expected_prefix="$2"
  if [[ -z "$path" || -z "$expected_prefix" ]]; then
    return 0
  fi
  case "$path" in
    "${expected_prefix}"*) ;;
    *)
      echo "web-e2e-real: refusing to remove unvalidated path" >&2
      return 1
      ;;
  esac
  if [[ -L "$path" ]]; then
    echo "web-e2e-real: refusing to remove symlink path" >&2
    return 1
  fi
  if [[ -d "$path" ]]; then
    rm -rf -- "$path"
  fi
}

parse_fixture_field() {
  local path="$1"
  local field="$2"
  python3 - "$path" "$field" <<'PY'
import json
import sys
from pathlib import Path

path = Path(sys.argv[1])
field = sys.argv[2]
payload = json.loads(path.read_text(encoding="utf-8"))
value = payload.get(field)
if not isinstance(value, str) or not value.strip():
    raise SystemExit(f"fixture field missing: {field}")
print(value.strip())
PY
}

load_secret_canaries_from_credentials() {
  local path="$1"
  if [[ ! -f "$path" ]]; then
    return 0
  fi
  python3 - "$path" <<'PY'
import json
import sys
from pathlib import Path

payload = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
values = []
for key in ("adminPassword", "viewerPassword", "adminEmail", "viewerEmail"):
    value = payload.get(key)
    if isinstance(value, str) and value.strip():
        values.append(value.strip())
print("\n".join(values))
PY
}

export_secret_canaries_from_credentials() {
  local path="$1"
  local canaries=""
  if [[ ! -f "$path" ]]; then
    return 0
  fi
  canaries="$(load_secret_canaries_from_credentials "$path" || true)"
  if [[ -z "$canaries" ]]; then
    return 0
  fi
  if [[ -n "${WEB_E2E_REAL_SECRET_CANARIES:-}" ]]; then
    export WEB_E2E_REAL_SECRET_CANARIES="${WEB_E2E_REAL_SECRET_CANARIES}"$'\n'"${canaries}"
  else
    export WEB_E2E_REAL_SECRET_CANARIES="$canaries"
  fi
}

# Merge the run content canary into WEB_E2E_REAL_CONTENT_CANARIES for artifact
# validation. Specs embed WEB_E2E_REAL_CONTENT_CANARY (default
# P2-20-CONTENT-CANARY) in unique upload bodies.
export_content_canaries() {
  local canary="${WEB_E2E_REAL_CONTENT_CANARY:-P2-20-CONTENT-CANARY}"
  export WEB_E2E_REAL_CONTENT_CANARY="$canary"
  if [[ -z "${WEB_E2E_REAL_CONTENT_CANARIES:-}" ]]; then
    export WEB_E2E_REAL_CONTENT_CANARIES="$canary"
    return 0
  fi
  case $'\n'"${WEB_E2E_REAL_CONTENT_CANARIES}"$'\n' in
    *$'\n'"${canary}"$'\n'*) ;;
    *)
      export WEB_E2E_REAL_CONTENT_CANARIES="${WEB_E2E_REAL_CONTENT_CANARIES}"$'\n'"${canary}"
      ;;
  esac
}

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
    if [[ -z "$path" || ! -f "$path" ]]; then
      echo "(no log file)" >&2
      continue
    fi
    if ! python3 "$REDACT" "$path" >&2; then
      redactor_failed=true
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
    # Server-only deterministic knobs must not leak into workers.
    unset MARKHAND_MAX_UPLOAD_BYTES MARKHAND_RATE_ROUTE_PER_MINUTE

    export MARKHAND_WORKER_DATABASE_URL
    export MARKHAND_WORKER_ORG_ID
    export MARKHAND_WORKER_USER_ID
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
    WEB_E2E_REAL_PLAYWRIGHT_RESULTS="$playwright_results" \
      bash -c "$WEB_E2E_REAL_PLAYWRIGHT_CMD" &
    playwright_pid=$!
  else
    MARKHAND_E2E_REAL=1 \
      MARKHAND_E2E_REAL_BASE_URL="http://${bind_addr}" \
      MARKHAND_E2E_REAL_FIXTURE_FILE="$fixture_file" \
      MARKHAND_E2E_REAL_CREDENTIALS_FILE="$credentials_file" \
      WEB_E2E_REAL_PLAYWRIGHT_RESULTS="$playwright_results" \
      pnpm --dir "$WEB_DIR" exec playwright test --project=real --reporter=json \
      >"$playwright_results" 2>"${playwright_results}.stderr" &
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

stop_and_reap_processes() {
  local worker_pid
  if [[ -n "$server_pid" ]]; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
    server_pid=""
  fi
  for worker_pid in "${worker_pids[@]}"; do
    kill "$worker_pid" 2>/dev/null || true
  done
  for worker_pid in "${worker_pids[@]}"; do
    wait "$worker_pid" 2>/dev/null || true
  done
  worker_pids=()
}

run_fixture_teardown() {
  # CI full-stack runs accumulate many org-scoped rows; 30s is too short after a
  # mid-suite Playwright failure and falsely reports cleanup interruption.
  local timeout_secs="${WEB_E2E_REAL_CLEANUP_TIMEOUT_SECS:-}"
  if [[ -z "$timeout_secs" ]]; then
    if [[ "${CI:-}" == "true" || "${CI:-}" == "1" ]]; then
      timeout_secs=180
    else
      timeout_secs=30
    fi
  fi
  local api_base="http://${bind_addr:-127.0.0.1:8787}"
  local cleanup_rc=0
  local verify_rc=0

  # Keep errexit off for the whole helper. A non-zero `return` under `set -e`
  # aborts the EXIT trap before status preservation / process reap can run.
  set +e

  if [[ "$fixture_ready" != true ]]; then
    teardown_result="ok"
    return 0
  fi

  python3 "$FIXTURE_CLI" cleanup \
    --run-id "$run_id" \
    --manifest "$fixture_file" \
    --credentials "$credentials_file" \
    --api-base "$api_base" \
    --timeout-secs "$timeout_secs"
  cleanup_rc=$?
  if [[ "$cleanup_rc" -ne 0 ]]; then
    teardown_result="failed"
    return "$cleanup_rc"
  fi

  python3 "$FIXTURE_CLI" verify-clean \
    --run-id "$run_id" \
    --manifest "$fixture_file"
  verify_rc=$?
  if [[ "$verify_rc" -ne 0 ]]; then
    teardown_result="failed"
    return "$verify_rc"
  fi

  teardown_result="ok"
  return 0
}

stage_and_validate_artifacts() {
  local manifest_out="${artifact_dir}/manifest.json"
  local write_rc=0
  local validate_rc=0

  # Keep errexit off for the whole helper so failing returns propagate to the
  # EXIT trap instead of terminating the shell under `set -e`.
  set +e

  if [[ -z "$artifact_dir" || -z "$playwright_results" || -z "$fixture_file" ]]; then
    artifact_failed=true
    return 1
  fi
  if [[ ! -f "$playwright_results" ]]; then
    artifact_failed=true
    echo "web-e2e-real: missing Playwright results for artifact staging" >&2
    return 1
  fi

  python3 "$ARTIFACTS_CLI" write \
    --results "$playwright_results" \
    --fixture "$fixture_file" \
    --out "$manifest_out" \
    --teardown "$teardown_result"
  write_rc=$?
  if [[ "$write_rc" -ne 0 ]]; then
    artifact_failed=true
    echo "web-e2e-real: artifact manifest write failed" >&2
    return "$write_rc"
  fi

  python3 "$ARTIFACTS_CLI" validate \
    --manifest "$manifest_out" \
    --artifact-dir "$artifact_dir"
  validate_rc=$?
  if [[ "$validate_rc" -ne 0 ]]; then
    artifact_failed=true
    echo "web-e2e-real: artifact validation failed" >&2
    return "$validate_rc"
  fi
  return 0
}

cleanup() {
  local original_status=$?
  local fixture_rc=0
  local artifact_rc=0
  local delay_secs="${WEB_E2E_REAL_CLEANUP_DELAY_SECS:-}"

  if [[ "$cleanup_started" == true ]]; then
    return 0
  fi
  cleanup_started=true
  trap - EXIT

  if [[ "$original_status" -ne 0 ]]; then
    upgrade_status "$original_status"
  fi

  if [[ -n "$delay_secs" ]]; then
    sleep "$delay_secs"
  fi

  # Capture credential canaries before cleanup deletes the 0600 credentials file.
  export_secret_canaries_from_credentials "$credentials_file"
  export_content_canaries

  # Fixture cleanup + verify-clean must run while server/delete worker are alive.
  set +e
  run_fixture_teardown
  fixture_rc=$?
  set -e
  if [[ "$fixture_rc" -ne 0 ]]; then
    upgrade_status "$fixture_rc"
    cleanup_status=1
  fi

  # Only stage/validate when Playwright results exist; setup-time failures should
  # not invent an artifact-validation primary status.
  if [[ -n "$playwright_results" && -f "$playwright_results" ]]; then
    set +e
    stage_and_validate_artifacts
    artifact_rc=$?
    set -e
    if [[ "$artifact_rc" -ne 0 ]]; then
      upgrade_status "$artifact_rc"
      cleanup_status=1
      artifact_failed=true
    fi
  elif [[ "$fixture_ready" == true ]]; then
    # Playwright never produced results after a fixture was created.
    artifact_failed=true
    upgrade_status 1
    cleanup_status=1
  fi

  if [[ "$redactor_failed" == true ]]; then
    upgrade_status 1
    cleanup_status=1
  fi

  stop_and_reap_processes

  if [[ -n "$server_log" ]]; then
    rm -f -- "$server_log" "$convert_log" "$index_log" "$embedding_log" "$delete_log"
  fi
  if [[ -n "$playwright_results" ]]; then
    rm -f -- "$playwright_results" "${playwright_results}.stderr"
  fi
  if [[ "$created_env" == true ]]; then
    rm -f -- "$ENV_FILE"
  fi
  if [[ "$created_runtime_dir" == true && -n "$runtime_dir" ]]; then
    safe_rm_tree "$runtime_dir" "${TMPDIR:-/tmp}/markhand-web-e2e-real-runtime." || cleanup_status=1
  fi
  # Caller-provided artifact dirs are retained; only auto-created temps are removed
  # after a fully successful run so CI can collect failure manifests.
  if [[ "$created_artifact_dir" == true && -n "$artifact_dir" && "$job_status" -eq 0 && "$cleanup_status" -eq 0 ]]; then
    safe_rm_tree "$artifact_dir" "${TMPDIR:-/tmp}/markhand-web-e2e-real-artifacts." || cleanup_status=1
  fi

  if [[ "$cleanup_status" -ne 0 && "$job_status" -eq 0 ]]; then
    job_status=1
  fi

  if [[ "$job_status" -ne 0 ]]; then
    exit "$job_status"
  fi
}

server_log="$(mktemp)"
convert_log="$(mktemp)"
index_log="$(mktemp)"
embedding_log="$(mktemp)"
delete_log="$(mktemp)"
trap cleanup EXIT

if [[ -z "${WEB_E2E_REAL_RUN_ID:-}" ]]; then
  run_id="$(python3 - <<'PY'
import uuid
print(uuid.uuid4())
PY
)"
else
  run_id="${WEB_E2E_REAL_RUN_ID}"
fi
export WEB_E2E_REAL_RUN_ID="$run_id"

# Content canary for Playwright upload bodies + artifact validation scan.
export_content_canaries

if [[ -n "${WEB_E2E_REAL_ARTIFACT_DIR:-}" ]]; then
  artifact_dir="${WEB_E2E_REAL_ARTIFACT_DIR}"
  mkdir -p "$artifact_dir"
else
  artifact_dir="$(mktemp -d "${TMPDIR:-/tmp}/markhand-web-e2e-real-artifacts.XXXXXX")"
  created_artifact_dir=true
fi
export WEB_E2E_REAL_ARTIFACT_DIR="$artifact_dir"

if [[ -n "${WEB_E2E_REAL_RUNTIME_DIR:-}" ]]; then
  runtime_dir="${WEB_E2E_REAL_RUNTIME_DIR}"
  mkdir -p "$runtime_dir"
else
  runtime_dir="$(mktemp -d "${TMPDIR:-/tmp}/markhand-web-e2e-real-runtime.XXXXXX")"
  created_runtime_dir=true
fi

# Credentials stay outside the staged artifact directory (mode 0600 via fixture CLI).
fixture_file="${runtime_dir}/fixture-manifest.json"
credentials_file="${runtime_dir}/fixture-credentials.json"
playwright_results="${runtime_dir}/playwright-results.json"
export MARKHAND_E2E_REAL_FIXTURE_FILE="$fixture_file"
export MARKHAND_E2E_REAL_CREDENTIALS_FILE="$credentials_file"
export WEB_E2E_REAL_PLAYWRIGHT_RESULTS="$playwright_results"

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

if [[ "$ORCHESTRATION_TEST" != "1" ]]; then
  "$ROOT/deploy/scripts/bootstrap-server-role.sh"
  "$ROOT/deploy/scripts/migrate.sh"
  python3 "$ROOT/deploy/poc/qdrant-init.py"
fi

# Run-scoped fixture after migrate/qdrant init and before workers/server.
python3 "$FIXTURE_CLI" setup \
  --run-id "$run_id" \
  --manifest-out "$fixture_file" \
  --credentials-out "$credentials_file"
fixture_ready=true

MARKHAND_WORKER_ORG_ID="$(parse_fixture_field "$fixture_file" "orgId")"
MARKHAND_WORKER_USER_ID="$(parse_fixture_field "$fixture_file" "adminUserId")"
export MARKHAND_WORKER_ORG_ID MARKHAND_WORKER_USER_ID
export MARKHAND_E2E_REAL_FIXTURE_FILE="$fixture_file"
export MARKHAND_E2E_REAL_CREDENTIALS_FILE="$credentials_file"

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

# Process-local server env for deterministic 413/429 (dev/CI only).
MARKHAND_MAX_UPLOAD_BYTES=4096 \
  MARKHAND_RATE_ROUTE_PER_MINUTE=1 \
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
    upgrade_status 1
    exit 1
  fi
  if ! kill -0 "$server_pid" 2>/dev/null; then
    dump_redacted_logs "fileconv-server exited during readiness"
    upgrade_status 1
    exit 1
  fi
  if ! required_processes_alive "during readiness"; then
    upgrade_status 1
    exit 1
  fi
  sleep 1
done

if [[ "$healthy" != true ]]; then
  dump_redacted_logs "fileconv-server never became ready"
  echo "unhealthy: fileconv-server" >&2
  upgrade_status 1
  exit 1
fi

if ! required_processes_alive "after readiness, before seed"; then
  upgrade_status 1
  exit 1
fi

"$ROOT/deploy/scripts/seed-dev-all.sh" --skip-init
echo "healthy: fileconv-server + convert/index/embedding/delete workers (web/dist from $MARKHAND_WEB_DIST_DIR)"

if ! required_processes_alive "after seed, before Playwright"; then
  upgrade_status 1
  exit 1
fi

dump_playwright_scenario_summary() {
  local results_path="$1"
  if [[ -z "$results_path" || ! -f "$results_path" ]]; then
    echo "web-e2e-real: no Playwright JSON results to summarize" >&2
    return 0
  fi
  python3 - "$results_path" <<'PY' || true
import json
import re
import sys
from pathlib import Path

path = Path(sys.argv[1])
try:
    payload = json.loads(path.read_text(encoding="utf-8"))
except Exception as error:
    print(f"web-e2e-real: unable to parse Playwright results ({error})", file=sys.stderr)
    raise SystemExit(0)

# Keep failure hints short and strip common secret-looking tokens.
_SECRETISH = re.compile(
    r"(?i)(bearer\s+[a-z0-9._\-+=/]+|eyJ[a-z0-9_\-]+=*\.[a-z0-9_\-.=]+|"
    r"mhcap1\.[a-z0-9._\-]+|password[=:]\s*\S+)"
)


def safe_error(text: str) -> str:
    cleaned = _SECRETISH.sub("[redacted]", text or "")
    cleaned = " ".join(cleaned.split())
    return cleaned[:240]


def walk(suite, prefix=""):
    title = suite.get("title") or ""
    path = f"{prefix} › {title}".strip(" ›") if title else prefix
    for spec in suite.get("specs") or []:
        spec_title = spec.get("title") or "(untitled)"
        full = f"{path} › {spec_title}".strip(" ›")
        outcome = "unknown"
        hint = ""
        for test in spec.get("tests") or []:
            results = test.get("results") or []
            if results:
                # Prefer the last attempt (retry-aware).
                last = results[-1]
                status = last.get("status")
                if status:
                    outcome = status
                for error in last.get("errors") or []:
                    message = error.get("message") if isinstance(error, dict) else str(error)
                    if message:
                        hint = safe_error(message)
                        break
                if not hint:
                    error = last.get("error")
                    if isinstance(error, dict) and error.get("message"):
                        hint = safe_error(str(error["message"]))
                    elif isinstance(error, str):
                        hint = safe_error(error)
            if outcome == "unknown":
                status = test.get("status")
                if status:
                    outcome = status
        line = f"  [{outcome}] {full}"
        if hint and outcome not in {"passed", "skipped", "expected"}:
            line = f"{line} :: {hint}"
        print(line)
    for child in suite.get("suites") or []:
        walk(child, path)

print("web-e2e-real: Playwright scenario summary (title/outcome + safe error hints):")
for suite in payload.get("suites") or []:
    walk(suite)
stats = payload.get("stats") or {}
if stats:
    print(
        "web-e2e-real: stats "
        f"expected={stats.get('expected')} unexpected={stats.get('unexpected')} "
        f"skipped={stats.get('skipped')} flaky={stats.get('flaky')}"
    )
PY
}

playwright_status=0
run_playwright_supervised || playwright_status=$?
set -e
if [[ "$playwright_status" -ne 0 ]]; then
  dump_playwright_scenario_summary "$playwright_results"
  dump_redacted_logs "Playwright real project failed (exit ${playwright_status})"
  upgrade_status "$playwright_status"
  exit "$playwright_status"
fi

exit 0
