#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT/deploy/dev"
if [[ -f .env ]]; then
  set -a
  # shellcheck disable=SC1091
  source .env
  set +a
fi
docker compose --profile aiteamvn --profile mock --profile gpu down --volumes --remove-orphans
"$ROOT/deploy/scripts/up.sh"
