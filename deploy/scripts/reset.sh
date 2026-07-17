#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT/deploy/dev"
docker compose down --volumes --remove-orphans
"$ROOT/deploy/scripts/up.sh"
