#!/usr/bin/env bash
# Validate recovery manifest structure, signature, versions, and artifact checksums.
# shellcheck shell=bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../lib/common.sh
source "$SCRIPT_DIR/../lib/common.sh"
markhand_backup_init
markhand_load_poc_env

MANIFEST="${1:-}"
BACKUP_ROOT="${2:-}"
if [[ -z "$MANIFEST" ]]; then
  markhand_die "usage: validate-manifest.sh <recovery-manifest.json> [backup-root]"
fi

ARGS=(
  "$MANIFEST_PY" validate
  --manifest "$MANIFEST"
  --verify-signature
)
if [[ -n "${MARKHAND_WORKER_ORG_ID:-}" ]]; then
  ARGS+=(--org-id "$MARKHAND_WORKER_ORG_ID")
fi
if [[ -n "${MARKHAND_BACKUP_SCHEMA_NAME:-}" ]]; then
  ARGS+=(--schema-name "$MARKHAND_BACKUP_SCHEMA_NAME")
else
  ARGS+=(--schema-name public)
fi
if [[ -n "${MARKHAND_INDEX_SIGNATURE:-}" ]]; then
  ARGS+=(--index-signature "$MARKHAND_INDEX_SIGNATURE")
fi
if [[ -n "${MARKHAND_BACKUP_MIGRATION_VERSION:-}" ]]; then
  ARGS+=(--migration-version "$MARKHAND_BACKUP_MIGRATION_VERSION")
fi
if [[ -n "$BACKUP_ROOT" ]]; then
  ARGS+=(--backup-root "$BACKUP_ROOT")
fi

python3 "${ARGS[@]}"
