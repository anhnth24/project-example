#!/usr/bin/env bash
# shellcheck shell=bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
MANIFEST="${1:?usage: validate-manifest.sh <manifest> [backup-root]}"
ARGS=(validate-manifest --manifest "$MANIFEST")
if [[ -n "${2:-}" ]]; then
  ARGS+=(--backup-root "$2")
fi
exec python3 "$ROOT/deploy/backup/lib/pipeline.py" "${ARGS[@]}"
