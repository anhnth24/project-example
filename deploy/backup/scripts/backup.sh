#!/usr/bin/env bash
# Orchestrate a consistency-fenced multi-store backup and signed recovery manifest.
# Idempotent/resumable via .state/stage checkpoints under the backup root.
# shellcheck shell=bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../lib/common.sh
source "$SCRIPT_DIR/../lib/common.sh"
markhand_backup_init
markhand_load_poc_env

BACKUP_DIR="${1:-}"
if [[ -z "$BACKUP_DIR" ]]; then
  markhand_die "usage: backup.sh <backup-root>"
fi
mkdir -p "$BACKUP_DIR"
BACKUP_DIR="$(cd "$BACKUP_DIR" && pwd -P)"

markhand_require_env MARKHAND_WORKER_ORG_ID
markhand_require_env MARKHAND_INDEX_SIGNATURE
markhand_require_env MARKHAND_BACKUP_SIGNING_KEY_ID
markhand_require_env MARKHAND_BACKUP_SIGNING_KEY
markhand_require_env MARKHAND_BACKUP_APP_VERSION
markhand_require_env MARKHAND_BACKUP_MIGRATION_VERSION

STAGE="$(markhand_checkpoint_get "$BACKUP_DIR")"
markhand_log "resume stage=$STAGE mode=$MARKHAND_BACKUP_MODE"

FENCE_JSON="$BACKUP_DIR/fence.json"
if [[ "$STAGE" == "none" || "$STAGE" == "fence"* ]]; then
  "$SCRIPT_DIR/fence-writes.sh" "$FENCE_JSON"
  markhand_checkpoint_set "$BACKUP_DIR" "fenced"
fi

if [[ "$(markhand_checkpoint_get "$BACKUP_DIR")" != "postgres-backed-up" \
   && "$(markhand_checkpoint_get "$BACKUP_DIR")" != "minio-backed-up" \
   && "$(markhand_checkpoint_get "$BACKUP_DIR")" != "qdrant-backed-up" \
   && "$(markhand_checkpoint_get "$BACKUP_DIR")" != "manifest-written" ]]; then
  "$SCRIPT_DIR/backup-postgres.sh" "$BACKUP_DIR"
fi

STAGE="$(markhand_checkpoint_get "$BACKUP_DIR")"
if [[ "$STAGE" == "postgres-backed-up" ]]; then
  "$SCRIPT_DIR/backup-minio.sh" "$BACKUP_DIR"
fi

STAGE="$(markhand_checkpoint_get "$BACKUP_DIR")"
if [[ "$STAGE" == "minio-backed-up" ]]; then
  "$SCRIPT_DIR/backup-qdrant.sh" "$BACKUP_DIR"
fi

STAGE="$(markhand_checkpoint_get "$BACKUP_DIR")"
if [[ "$STAGE" != "qdrant-backed-up" && "$STAGE" != "manifest-written" ]]; then
  markhand_die "backup incomplete; stage=$STAGE"
fi

# Assemble signed recovery manifest binding all three stores.
PG_META="$BACKUP_DIR/postgres/postgres-meta.json"
MINIO_META="$BACKUP_DIR/minio/minio-meta.json"
QDRANT_META="$BACKUP_DIR/qdrant/qdrant-meta.json"
for required in "$FENCE_JSON" "$PG_META" "$MINIO_META" "$QDRANT_META"; do
  [[ -f "$required" ]] || markhand_die "missing required meta: $required"
done

MANIFEST_OUT="$BACKUP_DIR/recovery-manifest.json"
python3 - <<PY
import json
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path("$BACKUP_LIB_DIR")))
from manifest import build_manifest, sha256_file, signing_key_from_env

backup_dir = Path("$BACKUP_DIR")
fence = json.loads((backup_dir / "fence.json").read_text(encoding="utf-8"))
postgres = json.loads((backup_dir / "postgres/postgres-meta.json").read_text(encoding="utf-8"))
minio = json.loads((backup_dir / "minio/minio-meta.json").read_text(encoding="utf-8"))
qdrant = json.loads((backup_dir / "qdrant/qdrant-meta.json").read_text(encoding="utf-8"))

relative_paths = {
    "postgres_base": "postgres/base.tar.enc",
    "postgres_wal_boundary": "postgres/wal-boundary.txt",
    "postgres_meta": "postgres/postgres-meta.json",
    "minio_inventory": "minio/version-inventory.tsv",
    "minio_meta": "minio/minio-meta.json",
    "qdrant_snapshot": "qdrant/snapshot.bin",
    "qdrant_meta": "qdrant/qdrant-meta.json",
    "fence": "fence.json",
}
checksums = {}
for rel in relative_paths.values():
    path = backup_dir / rel
    if not path.is_file():
        raise SystemExit(f"missing artifact for manifest: {rel}")
    checksums[rel] = sha256_file(path)

key_id, key = signing_key_from_env()
manifest_id = "rm-" + checksums[relative_paths["postgres_base"]][:16]
payload = build_manifest(
    manifest_id=manifest_id,
    org_id=os.environ["MARKHAND_WORKER_ORG_ID"],
    app_version=os.environ["MARKHAND_BACKUP_APP_VERSION"],
    migration_version=os.environ["MARKHAND_BACKUP_MIGRATION_VERSION"],
    index_signature=os.environ["MARKHAND_INDEX_SIGNATURE"],
    postgres=postgres,
    minio=minio,
    qdrant=qdrant,
    relative_paths=relative_paths,
    checksums=checksums,
    consistency_fence=fence,
    schema_name=os.environ.get("MARKHAND_BACKUP_SCHEMA_NAME", "public"),
    notes=[
        "Recovery manifest binds PG WAL boundary, MinIO inventory digest, Qdrant snapshot.",
        "Object keys/content and credentials are excluded by design.",
    ],
    key_id=key_id,
    key=key,
)
out = backup_dir / "recovery-manifest.json"
out.write_text(json.dumps(payload, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
print(out)
PY

"$SCRIPT_DIR/validate-manifest.sh" "$MANIFEST_OUT" "$BACKUP_DIR"
markhand_checkpoint_set "$BACKUP_DIR" "manifest-written"
markhand_log "backup complete manifest=$MANIFEST_OUT"
echo "$MANIFEST_OUT"
