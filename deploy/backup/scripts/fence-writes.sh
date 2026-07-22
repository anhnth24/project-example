#!/usr/bin/env bash
# shellcheck shell=bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
exec python3 "$ROOT/deploy/backup/lib/pipeline.py" fence --output "${1:?usage: fence-writes.sh <fence.json>}"
