#!/usr/bin/env bash
# Create deploy/dev/.env and worker.env from examples (never overwrites existing files).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DEV_DIR="$ROOT/deploy/dev"

copy_if_missing() {
  local target="$1"
  local example="$2"
  if [[ -f "$DEV_DIR/$target" ]]; then
    echo "keep existing: deploy/dev/$target"
    return
  fi
  if [[ ! -f "$DEV_DIR/$example" ]]; then
    echo "missing example: deploy/dev/$example" >&2
    exit 1
  fi
  cp "$DEV_DIR/$example" "$DEV_DIR/$target"
  echo "created deploy/dev/$target from $example"
}

copy_if_missing ".env" ".env.example"
copy_if_missing "worker.env" "worker.env.example"

# Ensure index signature matches embedding vars (idempotent append).
ENV_FILE="$DEV_DIR/.env"
if [[ -f "$ENV_FILE" ]] && ! grep -q '^MARKHAND_INDEX_SIGNATURE=' "$ENV_FILE"; then
  set -a
  # shellcheck disable=SC1090
  source "$ENV_FILE"
  set +a
  signature="$(python3 "$ROOT/deploy/scripts/print-index-signature.py")"
  {
    echo ""
    echo "# Appended by init-dev-env.sh — recompute: python3 deploy/scripts/print-index-signature.py"
    echo "MARKHAND_INDEX_SIGNATURE=$signature"
  } >>"$ENV_FILE"
  echo "appended MARKHAND_INDEX_SIGNATURE to deploy/dev/.env"
fi

echo ""
echo "Next: make dev-up && bootstrap-server-role.sh && start fileconv-server once, then:"
echo "  deploy/scripts/seed-dev-all.sh"
