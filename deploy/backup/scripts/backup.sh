#!/usr/bin/env bash
# Thin wrapper — all logic in deploy/backup/lib/pipeline.py (no secret heredocs).
# shellcheck shell=bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
exec python3 "$ROOT/deploy/backup/lib/pipeline.py" backup --backup-root "${1:?usage: backup.sh <backup-root>}"
