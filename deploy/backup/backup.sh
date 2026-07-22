#!/usr/bin/env bash
# Phase 1B blue/green backup capture (ADR 0012).
# Fail-closed: fence is mandatory; versioned object bytes + Qdrant snapshot bytes
# are bundled and checksummed. No placeholder success.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT_DIR="${MARKHAND_BACKUP_DIR:-$ROOT/tmp/markhand-backup}"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
DEST="$OUT_DIR/$STAMP"
mkdir -p "$DEST/objects"

: "${DATABASE_URL:?DATABASE_URL required}"
: "${MINIO_ENDPOINT:?MINIO_ENDPOINT required}"
: "${QDRANT_URL:?QDRANT_URL required}"
: "${MINIO_BUCKET:?MINIO_BUCKET required}"
: "${MINIO_ACCESS_KEY:?MINIO_ACCESS_KEY required}"
: "${MINIO_SECRET_KEY:?MINIO_SECRET_KEY required}"

die() { echo "backup failed: $*" >&2; exit 1; }

command -v pg_dump >/dev/null 2>&1 || die "pg_dump required"
command -v mc >/dev/null 2>&1 || die "mc (MinIO client) required"
command -v psql >/dev/null 2>&1 || die "psql required (mandatory ops fence)"
command -v curl >/dev/null 2>&1 || die "curl required"
command -v python3 >/dev/null 2>&1 || die "python3 required"

FENCE_FILE="$DEST/WRITE_FENCE"
echo "active blue/green fence at $STAMP — mutating traffic must pause" >"$FENCE_FILE"

# Mandatory durable fence — readiness reads the same ops_fences row.
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 <<SQL
INSERT INTO ops_fences (name, reason, active, set_at, cleared_at, set_by, attestation_sha256)
VALUES ('restore', 'backup capture $STAMP', true, now(), NULL, 'backup.sh', NULL)
ON CONFLICT (name) DO UPDATE
SET reason = EXCLUDED.reason,
    active = true,
    set_at = now(),
    cleared_at = NULL,
    set_by = EXCLUDED.set_by,
    attestation_sha256 = NULL;
SQL
test "$(psql "$DATABASE_URL" -Atc "SELECT active FROM ops_fences WHERE name='restore'")" = "t" \
  || die "ops_fences.restore not active after set"

pg_dump --no-owner --format=custom --file="$DEST/postgres.dump" "$DATABASE_URL" \
  || die "pg_dump failed"
test -s "$DEST/postgres.dump" || die "postgres.dump empty"

mc alias set markhand-backup "$MINIO_ENDPOINT" "$MINIO_ACCESS_KEY" "$MINIO_SECRET_KEY" >/dev/null
mc ls --recursive --versions "markhand-backup/${MINIO_BUCKET}" >"$DEST/minio-versions.txt" \
  || die "minio version inventory failed"
mc ls --recursive --json --versions "markhand-backup/${MINIO_BUCKET}" \
  >"$DEST/minio-versions.jsonl" || die "minio json version inventory failed"

# Bundle actual versioned object bytes into DEST/objects/ and checksum them.
python3 - <<'PY' "$DEST" "$MINIO_BUCKET" || die "object byte bundle failed"
import hashlib, json, pathlib, subprocess, sys
dest = pathlib.Path(sys.argv[1])
bucket = sys.argv[2]
src = dest / "minio-versions.jsonl"
entries = []
obj_dir = dest / "objects"
obj_dir.mkdir(parents=True, exist_ok=True)
for line in src.read_text(encoding="utf-8").splitlines():
    if not line.strip():
        continue
    row = json.loads(line)
    key = row.get("key") or row.get("Key")
    if not key or row.get("deleteMarker") or row.get("isDeleteMarker"):
        continue
    version = row.get("versionId") or row.get("version_id") or "null"
    cmd = ["mc", "cat", f"markhand-backup/{bucket}/{key}"]
    if version and version != "null":
        cmd = ["mc", "cat", "--version-id", str(version), f"markhand-backup/{bucket}/{key}"]
    proc = subprocess.run(cmd, check=False, capture_output=True)
    if proc.returncode != 0:
        raise SystemExit(f"mc cat failed for {key}@{version}")
    digest = hashlib.sha256(proc.stdout).hexdigest()
    safe = hashlib.sha256(f"{key}\0{version}".encode()).hexdigest()
    out = obj_dir / f"{safe}.bin"
    out.write_bytes(proc.stdout)
    entries.append(
        {
            "key": key,
            "versionId": version,
            "objectSha256": digest,
            "byteLength": len(proc.stdout),
            "bundleFile": f"objects/{safe}.bin",
        }
    )
(dest / "minio-object-checksums.json").write_text(
    json.dumps({"objects": entries, "bundled": True}, indent=2) + "\n",
    encoding="utf-8",
)
print(f"bundled {len(entries)} versioned object byte payloads")
PY

COLLECTION="${QDRANT_COLLECTION:-markhand}"
curl -fsS -X POST "${QDRANT_URL%/}/collections/${COLLECTION}/snapshots" \
  -o "$DEST/qdrant-snapshot-create.json" \
  || die "qdrant snapshot create failed"
python3 - <<'PY' "$DEST/qdrant-snapshot-create.json" "$DEST" "${QDRANT_URL}" "$COLLECTION" || die "qdrant snapshot download failed"
import json, pathlib, sys, urllib.request
meta_path, dest, base, collection = sys.argv[1:5]
meta = json.loads(pathlib.Path(meta_path).read_text(encoding="utf-8"))
if meta.get("ok") is False:
    raise SystemExit("qdrant snapshot ok=false")
result = meta.get("result") or {}
name = result.get("name")
if not name:
    raise SystemExit("qdrant snapshot name missing")
url = f"{base.rstrip('/')}/collections/{collection}/snapshots/{name}"
out = pathlib.Path(dest) / "qdrant-snapshot.bin"
with urllib.request.urlopen(url, timeout=120) as resp:
    data = resp.read()
if not data:
    raise SystemExit("qdrant snapshot bytes empty")
out.write_bytes(data)
(pathlib.Path(dest) / "qdrant-snapshot.name").write_text(name + "\n", encoding="utf-8")
print(f"downloaded snapshot {name} ({len(data)} bytes)")
PY
test -s "$DEST/qdrant-snapshot.bin" || die "qdrant-snapshot.bin empty"

APP_VERSION="$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)"
MIGRATION_VERSION="$(python3 - <<'PY'
import json
from pathlib import Path
manifest = json.loads(Path("crates/server/migrations/manifest.json").read_text())
print(sorted(manifest["migrations"])[-1])
PY
)"

python3 - <<PY
import hashlib, json, pathlib, sys
dest = pathlib.Path("$DEST")
files = [
    "postgres.dump",
    "minio-versions.txt",
    "minio-versions.jsonl",
    "minio-object-checksums.json",
    "qdrant-snapshot-create.json",
    "qdrant-snapshot.bin",
    "qdrant-snapshot.name",
    "WRITE_FENCE",
]
checksums = {}
for name in files:
    path = dest / name
    if not path.is_file():
        print(f"missing artifact: {name}", file=sys.stderr)
        raise SystemExit(1)
    if path.stat().st_size == 0 and name not in {
        "minio-versions.txt",
        "minio-versions.jsonl",
    }:
        print(f"empty artifact: {name}", file=sys.stderr)
        raise SystemExit(1)
    checksums[name] = hashlib.sha256(path.read_bytes()).hexdigest()
# Include every bundled object file.
objects = json.loads((dest / "minio-object-checksums.json").read_text())["objects"]
for entry in objects:
    rel = entry["bundleFile"]
    path = dest / rel
    if not path.is_file():
        raise SystemExit(f"missing bundled object: {rel}")
    actual = hashlib.sha256(path.read_bytes()).hexdigest()
    if actual != entry["objectSha256"]:
        raise SystemExit(f"bundle checksum mismatch: {rel}")
    checksums[rel] = actual
payload = {
  "capturedAt": "$STAMP",
  "appVersion": "$APP_VERSION",
  "migrationVersion": "$MIGRATION_VERSION",
  "mode": "blue_green",
  "fence": "WRITE_FENCE",
  "opsFence": "restore",
  "opsFenceMandatory": True,
  "stores": {
    "postgres": "postgres.dump",
    "minioVersions": "minio-versions.jsonl",
    "minioObjectChecksums": "minio-object-checksums.json",
    "minioObjectBytes": "objects/",
    "qdrantSnapshotBytes": "qdrant-snapshot.bin",
  },
  "artifactSha256": checksums,
  "rpoSecondsTarget": 900,
  "queryReadyRtoSecondsTarget": 3600,
  "status": "captured",
}
(dest / "manifest.json").write_text(json.dumps(payload, indent=2) + "\n")
digest = hashlib.sha256((dest / "manifest.json").read_bytes()).hexdigest()
(dest / "manifest.sha256").write_text(digest + "\n")
print(dest)
PY
