#!/usr/bin/env bash
# Validate P1B-F02 POC isolation scaffold without requiring a full GPU/runtime boot.
# Offline checks are authoritative on Docker-less hosts. Optional docker compose
# config / convert preflight run when a working Docker engine is available.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
COMPOSE_FILE="$ROOT/deploy/compose.poc.yml"
ENV_EXAMPLE="$ROOT/deploy/.env.example"
DOCKERFILE_SERVER="$ROOT/deploy/Dockerfile.server"
DOCKERFILE_WORKER="$ROOT/deploy/Dockerfile.worker"
IMAGES_LOCK="$ROOT/deploy/poc/images.lock.json"
FAIL=0

MOCK_SIG="72dda20007ffb7fbe293612091103321eb9e4e0e4a0517a5f3413e31a2978874"
AITEAMVN_SIG="dc6f6af4922063ae815fa3c84e17491b059d7c323fb8320d827f34386a038f86"
PDFIUM_SHA="e07bc44c4e422c50eb01da742dc1ec59ad6780ce64ed91955533da8e9fe1a363"

pass() { echo "PASS: $*"; }
fail() { echo "FAIL: $*" >&2; FAIL=1; }

require_file() {
  local path="$1"
  if [[ -f "$path" ]]; then
    pass "present $(basename "$path")"
  else
    fail "missing $path"
  fi
}

require_regex() {
  local path="$1"
  local pattern="$2"
  local label="$3"
  if grep -Eq -- "$pattern" "$path"; then
    pass "$label"
  else
    fail "$label (pattern not found in $(basename "$path"))"
  fi
}

forbid_regex() {
  local path="$1"
  local pattern="$2"
  local label="$3"
  if grep -Eqi -- "$pattern" "$path"; then
    fail "$label"
  else
    pass "$label"
  fi
}

echo "== P1B-F02 isolation smoke =="

require_file "$COMPOSE_FILE"
require_file "$ENV_EXAMPLE"
require_file "$DOCKERFILE_SERVER"
require_file "$DOCKERFILE_WORKER"
require_file "$ROOT/deploy/poc/minio-init.sh"
require_file "$ROOT/deploy/poc/minio-app-policy.json.tmpl"
require_file "$ROOT/deploy/poc/postgres-init.sh"
require_file "$IMAGES_LOCK"
require_file "$ROOT/deploy/poc/Dockerfile.embedding-cpu"
require_file "$ROOT/deploy/README.md"

# Separate API / worker images
require_regex "$COMPOSE_FILE" 'dockerfile: deploy/Dockerfile\.server' "api uses Dockerfile.server"
require_regex "$COMPOSE_FILE" 'dockerfile: deploy/Dockerfile\.worker' "workers use Dockerfile.worker"
require_regex "$DOCKERFILE_SERVER" 'useradd.*--uid 10001|--uid 10001' "server image non-root UID 10001"
require_regex "$DOCKERFILE_WORKER" 'useradd.*--uid 10001|--uid 10001' "worker image non-root UID 10001"

# Isolation flags on hardened services
for svc in api worker-convert worker-index worker-embedding; do
  if grep -q 'x-app-security: &app-security' "$COMPOSE_FILE" \
    && awk -v svc="$svc:" '
      $0 ~ "^  "svc {found=1; next}
      found && /^  [a-z0-9-]+:/ {exit}
      found && /<<: \*app-security/ {m=1}
      END { exit !(found && m) }
    ' "$COMPOSE_FILE"; then
    pass "$svc merges x-app-security (user/read_only/cap_drop/no-new-privileges)"
  else
    fail "$svc missing isolation hardening"
  fi
done

require_regex "$COMPOSE_FILE" 'tmpfs:' "tmpfs present for ephemeral scratch"
require_regex "$COMPOSE_FILE" 'mem_limit:' "resource mem_limit present"
require_regex "$COMPOSE_FILE" 'pids_limit:' "resource pids_limit present"
require_regex "$COMPOSE_FILE" 'cpus:' "resource cpus present"

# Convert no-egress network + sandbox-compatible hardening
require_regex "$COMPOSE_FILE" 'convert:' "convert network defined"
require_regex "$COMPOSE_FILE" 'internal:[[:space:]]*true' "convert network is internal (no egress)"
if awk '
  /^  worker-convert:/ {found=1; next}
  found && /^  [a-z0-9-]+:/ {exit}
  found && /networks:[[:space:]]*\[convert\]/ {net=1}
  END { exit !(found && net) }
' "$COMPOSE_FILE"; then
  pass "worker-convert attached only to convert network"
else
  fail "worker-convert must use networks: [convert] only"
fi
require_regex "$COMPOSE_FILE" 'seccomp=unconfined' \
  "convert worker relaxes seccomp for sandbox preflight (keeps cap_drop/no-egress)"
require_regex "$COMPOSE_FILE" '--sandbox-preflight' "convert worker healthcheck probes sandbox preflight"
require_regex "$ROOT/crates/server/src/workers/sandbox.rs" '/opt/pdfium' "sandbox landlock allows PDFium"
require_regex "$ROOT/crates/server/src/workers/sandbox.rs" 'tesseract-ocr|tessdata' \
  "sandbox landlock allows Tesseract data"
require_regex "$ROOT/crates/server/src/workers/sandbox.rs" 'FILECONV_PDFIUM_LIB' \
  "sandbox passes PDFium env into converter"
require_regex "$ROOT/crates/server/src/bin/worker.rs" '--sandbox-preflight' \
  "worker exposes --sandbox-preflight"

# Narrow MinIO credentials + fail-closed policy install
require_regex "$ENV_EXAMPLE" 'MARKHAND_MINIO_ROOT_USER' "root MinIO credentials separated"
require_regex "$ENV_EXAMPLE" 'MARKHAND_MINIO_ACCESS_KEY' "app MinIO access key present"
require_regex "$ROOT/deploy/poc/minio-app-policy.json.tmpl" '__BUCKET__' \
  "MinIO policy template is bucket-aware"
require_regex "$ROOT/deploy/poc/minio-init.sh" 'failed to install MinIO policy|failed to attach MinIO policy' \
  "MinIO init fail-closed on policy errors"
forbid_regex "$ROOT/deploy/poc/minio-init.sh" 'policy create.*\|\| true' \
  "MinIO policy create must not ignore errors"

# PhoWhisper exclusion + pinned PDFium
forbid_regex "$DOCKERFILE_WORKER" '^(COPY|ADD).*[Pp]ho[Ww]hisper' \
  "worker Dockerfile must not COPY/ADD PhoWhisper artifacts"
require_regex "$DOCKERFILE_WORKER" 'test ! -e /models/ggml-PhoWhisper-small.bin' \
  "worker Dockerfile guards against PhoWhisper model path"
forbid_regex "$DOCKERFILE_WORKER" 'releases/latest' "worker Dockerfile must not use releases/latest"
require_regex "$DOCKERFILE_WORKER" "$PDFIUM_SHA" "worker Dockerfile pins PDFium sha256"
require_regex "$DOCKERFILE_WORKER" 'chromium%2F7906|chromium/7906' "worker Dockerfile pins PDFium tag"
require_regex "$IMAGES_LOCK" "$PDFIUM_SHA" "images.lock records PDFium sha256"
require_regex "$DOCKERFILE_SERVER" 'rust:1\.88\.0-bookworm@sha256:' "server builder base digest pinned"
require_regex "$DOCKERFILE_SERVER" 'debian:bookworm-slim@sha256:' "server runtime base digest pinned"
require_regex "$DOCKERFILE_WORKER" 'rust:1\.88\.0-bookworm@sha256:' "worker builder base digest pinned"
require_regex "$DOCKERFILE_WORKER" 'debian:bookworm-slim@sha256:' "worker runtime base digest pinned"
require_regex "$DOCKERFILE_WORKER" 'tesseract-ocr=5\.3\.0-2' "worker pins tesseract-ocr apt version"
require_regex "$ROOT/deploy/poc/Dockerfile.embedding-cpu" \
  'python:3\.12\.12-slim-bookworm@sha256:' \
  "embedding-cpu base digest pinned"
require_regex "$IMAGES_LOCK" 'rust-bookworm' "images.lock records rust base"
require_regex "$IMAGES_LOCK" 'debian-bookworm-slim' "images.lock records debian base"
require_regex "$IMAGES_LOCK" 'python-slim-bookworm' "images.lock records python base"
require_regex "$IMAGES_LOCK" 'tesseract_apt' "images.lock records tesseract apt pins"

# Index signatures (source of truth: print-index-signature.py)
require_regex "$ENV_EXAMPLE" "$MOCK_SIG" ".env.example has mock index signature"
require_regex "$ENV_EXAMPLE" "$AITEAMVN_SIG" ".env.example documents AITeamVN index signature"
require_regex "$COMPOSE_FILE" "$MOCK_SIG" "compose defaults to mock index signature"
require_regex "$IMAGES_LOCK" "$MOCK_SIG" "images.lock records mock signature"
require_regex "$IMAGES_LOCK" "$AITEAMVN_SIG" "images.lock records AITeamVN signature"

if command -v python3 >/dev/null 2>&1 || command -v python >/dev/null 2>&1; then
  PYTHON_BIN="$(command -v python3 || command -v python)"
  computed_mock="$("$PYTHON_BIN" "$ROOT/deploy/scripts/print-index-signature.py" \
    --base-url http://mock-embedding:8080/v1 \
    --model markhand-mock --revision poc-local --dimensions 8)"
  computed_vn="$("$PYTHON_BIN" "$ROOT/deploy/scripts/print-index-signature.py" \
    --base-url http://embedding-cpu:8080/v1 \
    --model AITeamVN/Vietnamese_Embedding \
    --revision dea33aa1ab339f38d66ae0a40e6c40e0a9249568 --dimensions 1024)"
  if [[ "$computed_mock" == "$MOCK_SIG" ]]; then
    pass "print-index-signature.py mock matches $MOCK_SIG"
  else
    fail "print-index-signature.py mock produced $computed_mock"
  fi
  if [[ "$computed_vn" == "$AITEAMVN_SIG" ]]; then
    pass "print-index-signature.py AITeamVN matches $AITEAMVN_SIG"
  else
    fail "print-index-signature.py AITeamVN produced $computed_vn"
  fi

  MOCK_PY="$ROOT/deploy/scripts/mock-embedding.py"
  if "$PYTHON_BIN" - "$MOCK_PY" <<'PY'
import importlib.util
import math
import sys

spec = importlib.util.spec_from_file_location("mock_embedding", sys.argv[1])
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
vec = mod.embedding_for("tiếng Việt", 8)
norm = math.sqrt(sum(v * v for v in vec))
assert abs(norm - 1.0) <= 0.001, norm
assert any(abs(v) > 1e-6 for v in vec), vec
vec2 = mod.embedding_for("tiếng Việt", 8)
assert vec == vec2
print("ok")
PY
  then
    pass "mock-embedding returns deterministic L2-normalized vectors"
  else
    fail "mock-embedding L2 normalization check"
  fi
else
  echo "SKIP: python not available for signature/mock vector checks"
fi

# Postgres init parameterized
require_regex "$ROOT/deploy/poc/postgres-init.sh" 'MARKHAND_APP_DB_USER' \
  "postgres init reads app role from env"
require_regex "$ROOT/deploy/poc/postgres-init.sh" 'MARKHAND_APP_DB_PASSWORD' \
  "postgres init reads app password from env"
require_regex "$COMPOSE_FILE" 'postgres-init\.sh' "compose mounts parameterized postgres-init.sh"

# Embedding-cpu hardening
require_regex "$COMPOSE_FILE" 'deploy/poc/Dockerfile\.embedding-cpu' \
  "POC embedding uses hardened Dockerfile"
require_regex "$COMPOSE_FILE" 'user: "10001:10001"' "embedding-cpu runs as UID 10001"
require_regex "$ROOT/deploy/poc/Dockerfile.embedding-cpu" '--uid 10001' \
  "embedding Dockerfile creates non-root user"

# Pinned third-party digests
require_regex "$COMPOSE_FILE" 'postgres:18\.4-bookworm@sha256:' "postgres digest pinned"
require_regex "$COMPOSE_FILE" 'qdrant/qdrant:v1\.18\.2@sha256:' "qdrant digest pinned"
require_regex "$COMPOSE_FILE" 'pgsty/minio:.*@sha256:' "minio digest pinned"
require_regex "$COMPOSE_FILE" 'minio/mc:.*@sha256:' "minio/mc digest pinned"

# Embedding profiles (mock default, AITeamVN optional — not GLM)
require_regex "$COMPOSE_FILE" 'profiles: \["mock"\]' "mock embedding profile"
require_regex "$COMPOSE_FILE" 'profiles: \["aiteamvn"\]' "aiteamvn embedding profile"
require_regex "$ENV_EXAMPLE" 'COMPOSE_PROFILES=mock' "default profile is mock"
forbid_regex "$COMPOSE_FILE" 'glm|zhipu' "compose must not wire GLM embedding"

# API readiness (not only live)
require_regex "$COMPOSE_FILE" 'health/ready' "api healthcheck uses readiness"
require_regex "$ROOT/deploy/scripts/poc-health.sh" 'health/ready' "poc-health checks readiness"

# Optional docker compose config validation.
docker_compose_usable=false
if command -v docker >/dev/null 2>&1 && command -v timeout >/dev/null 2>&1; then
  if timeout 5 docker compose version >/dev/null 2>&1; then
    docker_compose_usable=true
  fi
fi
if [[ "$docker_compose_usable" == true ]]; then
  TMP_ENV="$ROOT/deploy/.env.poc-smoke.tmp"
  cp "$ENV_EXAMPLE" "$TMP_ENV"
  if COMPOSE_PROFILES=mock docker compose --env-file "$TMP_ENV" -f "$COMPOSE_FILE" config >/dev/null 2>"$TMP_ENV.err"; then
    pass "docker compose config (mock profile)"
  else
    fail "docker compose config (mock profile)"
    sed -n '1,40p' "$TMP_ENV.err" >&2 || true
  fi
  rm -f "$TMP_ENV" "$TMP_ENV.err"
else
  echo "SKIP: docker compose not probed — offline isolation checks are authoritative"
  echo "NOTE: full convert --sandbox-preflight / poc-up runtime proof requires Docker"
fi

if [[ "$FAIL" -ne 0 ]]; then
  echo "POC isolation smoke FAILED" >&2
  exit 1
fi
echo "POC isolation smoke PASSED"
echo "Catalog status must stay non-Done until Docker runtime boot/preflight evidence exists."
