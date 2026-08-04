#!/usr/bin/env bash
# Phase 1C G1C-SEC deployed qualification orchestrator.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
export ROOT
cd "$ROOT"

OUTPUT_DIR="${MARKHAND_PHASE1C_OUTPUT_DIR:-}"
SELF_TEST=0
VALIDATE_ARGS=0
declare -A SEEN_OPTS=()

usage() {
  echo "usage: g1c-security-gate.sh [--output-dir DIR] [--self-test]" >&2
}

validate_output_dir() {
  local dir="$1"
  if [[ -z "$dir" ]]; then
    echo "missing output directory; pass --output-dir or set MARKHAND_PHASE1C_OUTPUT_DIR" >&2
    exit 1
  fi
  if [[ "$dir" == *".."* ]]; then
    echo "unsafe output directory (path traversal rejected)" >&2
    exit 1
  fi
  if [[ "$dir" != /* ]]; then
    echo "output directory must be absolute" >&2
    exit 1
  fi
  if [[ -e "$dir" && -L "$dir" ]]; then
    echo "output directory must not be a symlink" >&2
    exit 1
  fi
  local probe="$dir"
  while [[ "$probe" != "/" ]]; do
    if [[ -L "$probe" ]]; then
      echo "output directory must not traverse symlinks" >&2
      exit 1
    fi
    probe="$(dirname "$probe")"
  done
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --output-dir)
      [[ $# -ge 2 ]] || { echo "missing value for --output-dir" >&2; exit 1; }
      if [[ -n "${SEEN_OPTS[output-dir]:-}" ]]; then
        echo "duplicate --output-dir" >&2
        exit 1
      fi
      SEEN_OPTS[output-dir]=1
      OUTPUT_DIR="$2"
      shift 2
      ;;
    --self-test)
      if [[ -n "${SEEN_OPTS[self-test]:-}" ]]; then
        echo "duplicate --self-test" >&2
        exit 1
      fi
      SEEN_OPTS[self-test]=1
      SELF_TEST=1
      shift
      ;;
    --validate-args)
      if [[ -n "${SEEN_OPTS[validate-args]:-}" ]]; then
        echo "duplicate --validate-args" >&2
        exit 1
      fi
      SEEN_OPTS[validate-args]=1
      VALIDATE_ARGS=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown option: $1" >&2
      usage
      exit 1
      ;;
  esac
done

if [[ "$SELF_TEST" -eq 1 ]]; then
  exec python3 bench/markhand_web/scripts/test_run_phase1c_gate.py
fi

if [[ "$VALIDATE_ARGS" -eq 1 ]]; then
  validate_output_dir "$OUTPUT_DIR"
  echo "phase1c-gate-args-ok"
  exit 0
fi

validate_output_dir "$OUTPUT_DIR"

: "${MARKHAND_TEST_REQUIRED:=1}"
: "${MARKHAND_PHASE1C_GATE:=1}"
: "${COMPOSE_PROFILES:=mock}"
export MARKHAND_TEST_REQUIRED MARKHAND_PHASE1C_GATE COMPOSE_PROFILES

if [[ "$COMPOSE_PROFILES" != *mock* ]]; then
  echo "G1C qualifying run requires mock embedding profile (COMPOSE_PROFILES=mock)" >&2
  exit 1
fi

mkdir -p "$OUTPUT_DIR"

# POC credentials for deployed probes (deploy/.env.example defaults).
export MARKHAND_TEST_DATABASE_URL="${MARKHAND_TEST_DATABASE_URL:-postgresql://markhand:markhand_poc_change_me@127.0.0.1:54330/markhand}"
export MARKHAND_TEST_APP_DATABASE_URL="${MARKHAND_TEST_APP_DATABASE_URL:-postgresql://markhand_app:markhand_app_poc_change_me@127.0.0.1:54330/markhand}"
export MARKHAND_TEST_MINIO_ENDPOINT="${MARKHAND_TEST_MINIO_ENDPOINT:-http://127.0.0.1:9010}"
export MARKHAND_TEST_MINIO_ACCESS_KEY="${MARKHAND_TEST_MINIO_ACCESS_KEY:-markhand_root}"
export MARKHAND_TEST_MINIO_SECRET_KEY="${MARKHAND_TEST_MINIO_SECRET_KEY:-markhand_root_poc_change_me}"
export MARKHAND_TEST_MINIO_REGION="${MARKHAND_TEST_MINIO_REGION:-us-east-1}"
export MARKHAND_TEST_QDRANT_URL="${MARKHAND_TEST_QDRANT_URL:-http://127.0.0.1:6343}"
export MARKHAND_TEST_QDRANT_ADMIN_API_KEY="${MARKHAND_TEST_QDRANT_ADMIN_API_KEY:-test-operator-admin-key}"

echo "G1C security gate (MARKHAND_PHASE1C_GATE=${MARKHAND_PHASE1C_GATE})"

# Seed POC admin password when deploy/.env exists (same pattern as O04 gate).
if [[ -f deploy/.env ]]; then
  # shellcheck disable=SC1091
  set -a && source deploy/.env && set +a
fi
if [[ -n "${MARKHAND_O04_API_PASSWORD:-}" ]]; then
  hash="$(cargo run -q -p fileconv-server --bin dev-hash-password -- "$MARKHAND_O04_API_PASSWORD")"
  hash_sql="${hash//\'/\'\'}"
  docker compose -f deploy/compose.poc.yml exec -T postgres psql \
    -U "${MARKHAND_POSTGRES_USER:-markhand}" -d "${MARKHAND_POSTGRES_DB:-markhand}" \
    --set ON_ERROR_STOP=1 \
    -c "UPDATE users SET password_hash = '${hash_sql}', updated_at = now() WHERE email = 'admin@poc.example';" \
    >/dev/null
fi

SEED_JSON="${MARKHAND_PHASE1C_SEED_JSON:-$ROOT/.artifacts/phase1c-multi-org-seed.json}"
export MARKHAND_PHASE1C_SEED_JSON="$SEED_JSON"
export MARKHAND_PHASE1C_CREDENTIALS_JSON="${MARKHAND_PHASE1C_CREDENTIALS_JSON:-$ROOT/.artifacts/phase1c-multi-org-seed.credentials.json}"

GATE_CRED_OK=0
purge_phase1c_credentials() {
  python3 - <<'PY'
import os
import sys
from pathlib import Path

root = Path(os.environ["ROOT"])
sys.path.insert(0, str(root / "bench/markhand_web/scripts"))
from phase1c_deployed_probes import purge_phase1c_credentials

cred_path = Path(os.environ.get("MARKHAND_PHASE1C_CREDENTIALS_JSON", ""))
if cred_path:
    purge_phase1c_credentials(cred_path)
PY
}
trap '[[ "$GATE_CRED_OK" == "1" ]] || purge_phase1c_credentials' EXIT HUP INT TERM

bash deploy/scripts/phase1c-multi-org-seed.sh

export MARKHAND_PHASE1C_CHALLENGE="$(
  python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["challenge"])' "$SEED_JSON"
)" || { echo "seed challenge parse failed" >&2; exit 1; }

cargo build -p fileconv-cli --no-default-features

python3 bench/markhand_web/scripts/run_phase1c_gate.py --output-dir "$OUTPUT_DIR"

GATE_CRED_OK=1

exec env \
  MARKHAND_PHASE1C_GATE=1 \
  MARKHAND_PHASE1C_REPORT_PATH="$OUTPUT_DIR/phase-1c-gate.json" \
  GITHUB_SHA="${GITHUB_SHA:-$(git -C "$ROOT" rev-parse HEAD)}" \
  cargo test -p fileconv-server --test e2e_phase1c_gate -- --ignored e2e_phase1c_gate --nocapture
