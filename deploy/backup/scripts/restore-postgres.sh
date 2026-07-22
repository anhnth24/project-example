#!/usr/bin/env bash
# Deprecated direct entry — use restore.sh (staged). Kept as fail-closed pointer.
# shellcheck shell=bash
set -euo pipefail
echo "error: use deploy/backup/scripts/restore.sh (staged PG→MinIO→Qdrant)" >&2
exit 2
