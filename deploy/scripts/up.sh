#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT/deploy/dev"
docker compose up -d
docker compose wait minio-init
"$ROOT/deploy/scripts/health.sh"
"$ROOT/deploy/scripts/seed.sh"
