#!/usr/bin/env bash
# shellcheck shell=bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
exec python3 "$ROOT/deploy/backup/lib/pipeline.py" backup-qdrant --backup-root "${1:?}"
