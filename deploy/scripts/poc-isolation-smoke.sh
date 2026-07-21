#!/usr/bin/env bash
# Validate P1B-F02 POC isolation scaffold without requiring a full GPU/runtime boot.
# Runs offline against compose YAML + Dockerfiles; optionally runs `docker compose config`
# when Docker is available.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
COMPOSE_FILE="$ROOT/deploy/compose.poc.yml"
ENV_EXAMPLE="$ROOT/deploy/.env.example"
DOCKERFILE_SERVER="$ROOT/deploy/Dockerfile.server"
DOCKERFILE_WORKER="$ROOT/deploy/Dockerfile.worker"
FAIL=0

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
  if grep -Eq "$pattern" "$path"; then
    pass "$label"
  else
    fail "$label (pattern not found in $(basename "$path"))"
  fi
}

forbid_regex() {
  local path="$1"
  local pattern="$2"
  local label="$3"
  if grep -Eqi "$pattern" "$path"; then
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
require_file "$ROOT/deploy/poc/minio-app-policy.json"
require_file "$ROOT/deploy/poc/postgres-init.sql"
require_file "$ROOT/deploy/README.md"

# Separate API / worker images
require_regex "$COMPOSE_FILE" 'dockerfile: deploy/Dockerfile\.server' "api uses Dockerfile.server"
require_regex "$COMPOSE_FILE" 'dockerfile: deploy/Dockerfile\.worker' "workers use Dockerfile.worker"
require_regex "$COMPOSE_FILE" 'MARKHAND_API_IMAGE|markhand-api:poc' "api image name pinned/overridable"
require_regex "$COMPOSE_FILE" 'MARKHAND_WORKER_IMAGE|markhand-worker:poc' "worker image name pinned/overridable"
require_regex "$DOCKERFILE_SERVER" 'fileconv-server' "server image builds fileconv-server"
require_regex "$DOCKERFILE_WORKER" 'fileconv-worker' "worker image builds fileconv-worker"
require_regex "$DOCKERFILE_SERVER" 'useradd.*10001|uid 10001|--uid 10001' "server image non-root UID 10001"
require_regex "$DOCKERFILE_WORKER" 'useradd.*10001|uid 10001|--uid 10001' "worker image non-root UID 10001"

# Isolation flags on hardened services
for svc in api worker-convert worker-index worker-embedding; do
  if awk -v svc="$svc:" '
    $0 ~ "^  "svc {found=1; next}
    found && /^  [a-z0-9-]+:/ {exit}
    found && /read_only:[[:space:]]*true/ {ro=1}
    found && /cap_drop:/ {cap=1}
    found && /no-new-privileges/ {np=1}
    found && /user:[[:space:]]*"10001:10001"/ {user=1}
    END { exit !(found && ro && cap && np && user) }
  ' "$COMPOSE_FILE"; then
    pass "$svc has user/read_only/cap_drop/no-new-privileges"
  else
    # Anchor merge: security may come from x-app-security <<: *app-security
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
  fi
done

require_regex "$COMPOSE_FILE" 'tmpfs:' "tmpfs present for ephemeral scratch"
require_regex "$COMPOSE_FILE" 'mem_limit:' "resource mem_limit present"
require_regex "$COMPOSE_FILE" 'pids_limit:' "resource pids_limit present"
require_regex "$COMPOSE_FILE" 'cpus:' "resource cpus present"

# Convert no-egress network
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

# Narrow MinIO credentials + no PhoWhisper bundle
require_regex "$ENV_EXAMPLE" 'MARKHAND_MINIO_ROOT_USER' "root MinIO credentials separated"
require_regex "$ENV_EXAMPLE" 'MARKHAND_MINIO_ACCESS_KEY' "app MinIO access key present"
require_regex "$ROOT/deploy/poc/minio-app-policy.json" 'quarantine/\*|trusted/\*' "MinIO policy scoped to prefixes"
forbid_regex "$DOCKERFILE_WORKER" '^(COPY|ADD).*[Pp]ho[Ww]hisper' \
  "worker Dockerfile must not COPY/ADD PhoWhisper artifacts"
require_regex "$DOCKERFILE_WORKER" 'test ! -e /models/ggml-PhoWhisper-small.bin' \
  "worker Dockerfile guards against PhoWhisper model path"
forbid_regex "$ENV_EXAMPLE" 'ggml-PhoWhisper' ".env.example must not reference PhoWhisper model path as bundled"
require_regex "$ENV_EXAMPLE" 'PhoWhisper' ".env.example documents PhoWhisper exclusion"

# Pinned third-party digests (reuse spike pins)
require_regex "$COMPOSE_FILE" 'postgres:18\.4-bookworm@sha256:' "postgres digest pinned"
require_regex "$COMPOSE_FILE" 'qdrant/qdrant:v1\.18\.2@sha256:' "qdrant digest pinned"
require_regex "$COMPOSE_FILE" 'pgsty/minio:.*@sha256:' "minio digest pinned"
require_regex "$COMPOSE_FILE" 'minio/mc:.*@sha256:' "minio/mc digest pinned"

# Embedding profiles (mock default, AITeamVN optional — not GLM)
require_regex "$COMPOSE_FILE" 'profiles: \["mock"\]' "mock embedding profile"
require_regex "$COMPOSE_FILE" 'profiles: \["aiteamvn"\]' "aiteamvn embedding profile"
require_regex "$ENV_EXAMPLE" 'COMPOSE_PROFILES=mock' "default profile is mock"
forbid_regex "$COMPOSE_FILE" 'glm|zhipu' "compose must not wire GLM embedding"

# Optional docker compose config validation.
# Offline YAML checks above are enough for CI without Docker. When Docker is
# present, prefer a bounded probe so a broken WSL stub cannot hang the script.
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
fi

if [[ "$FAIL" -ne 0 ]]; then
  echo "POC isolation smoke FAILED" >&2
  exit 1
fi
echo "POC isolation smoke PASSED"
