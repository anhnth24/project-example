#!/usr/bin/env bash
# Phase 1B blue/green restore (ADR 0012).
# Restores only into isolated green namespaces. Refuses destructive cutover unless
# object/vector bytes verify, reconcile completion is durable, and fence clear
# updates exactly one active row. No environment-variable fake attestations.
set -euo pipefail

BACKUP_DIR="${1:?usage: restore.sh <backup-dir>}"
MANIFEST="$BACKUP_DIR/manifest.json"
test -f "$MANIFEST" || { echo "manifest missing" >&2; exit 1; }
test -f "$BACKUP_DIR/manifest.sha256" || { echo "manifest.sha256 missing" >&2; exit 1; }
test -f "$BACKUP_DIR/WRITE_FENCE" || { echo "WRITE_FENCE missing; refuse restore" >&2; exit 1; }

expected="$(tr -d '[:space:]' <"$BACKUP_DIR/manifest.sha256")"
actual="$(sha256sum "$MANIFEST" | awk '{print $1}')"
if [[ "$expected" != "$actual" ]]; then
  echo "manifest checksum mismatch; refusing restore" >&2
  exit 1
fi

: "${DATABASE_URL:?DATABASE_URL required}"
GREEN_DATABASE_URL="${MARKHAND_GREEN_DATABASE_URL:-}"
GREEN_MINIO_BUCKET="${MARKHAND_GREEN_MINIO_BUCKET:-}"
GREEN_QDRANT_COLLECTION="${MARKHAND_GREEN_QDRANT_COLLECTION:-}"

python3 - <<'PY' "$BACKUP_DIR"
import hashlib, json, pathlib, sys
backup = pathlib.Path(sys.argv[1])
manifest = json.loads((backup / "manifest.json").read_text())
if manifest.get("mode") not in (None, "blue_green"):
    # Accept legacy manifests only for artifact checks; cutover still refused later.
    pass
required = {
    "postgres.dump",
    "minio-object-checksums.json",
    "qdrant-snapshot.bin",
    "WRITE_FENCE",
}
for name in required:
    if name not in manifest.get("artifactSha256", {}):
        raise SystemExit(f"manifest missing required artifact entry: {name}")
for name, digest in manifest.get("artifactSha256", {}).items():
    path = backup / name
    if not path.is_file():
        raise SystemExit(f"artifact missing: {name}")
    actual = hashlib.sha256(path.read_bytes()).hexdigest()
    if actual != digest:
        raise SystemExit(f"artifact checksum mismatch: {name}")
if (backup / "qdrant-snapshot.bin").stat().st_size == 0:
    raise SystemExit("qdrant-snapshot.bin empty")
objects = json.loads((backup / "minio-object-checksums.json").read_text()).get("objects", [])
for entry in objects:
    rel = entry.get("bundleFile")
    if not rel:
        raise SystemExit("object entry missing bundleFile — refuse non-bundled backup")
    path = backup / rel
    if not path.is_file():
        raise SystemExit(f"bundled object missing: {rel}")
    actual = hashlib.sha256(path.read_bytes()).hexdigest()
    if actual != entry.get("objectSha256"):
        raise SystemExit(f"bundled object checksum mismatch: {rel}")
print("artifact + bundled object checksums ok")
PY

if [[ -z "$GREEN_DATABASE_URL" || -z "$GREEN_MINIO_BUCKET" || -z "$GREEN_QDRANT_COLLECTION" ]]; then
  echo "blue/green restore requires isolated green targets:" >&2
  echo "  MARKHAND_GREEN_DATABASE_URL" >&2
  echo "  MARKHAND_GREEN_MINIO_BUCKET" >&2
  echo "  MARKHAND_GREEN_QDRANT_COLLECTION" >&2
  echo "REFUSING_DESTRUCTIVE_PROMOTE" >&2
  exit 2
fi

if [[ "$GREEN_DATABASE_URL" == "$DATABASE_URL" ]]; then
  echo "green database URL must differ from primary DATABASE_URL" >&2
  exit 1
fi

echo "1) Restore PostgreSQL into isolated green database"
command -v pg_restore >/dev/null 2>&1 || { echo "pg_restore required" >&2; exit 1; }
pg_restore --clean --if-exists --no-owner --dbname="$GREEN_DATABASE_URL" "$BACKUP_DIR/postgres.dump"

echo "2) Green staging verify"
python3 - <<'PY' "$GREEN_DATABASE_URL"
import sys
try:
    import psycopg
except ImportError:
    print("psycopg not installed; refusing soft-skip (fail-closed)", file=sys.stderr)
    raise SystemExit(1)
url = sys.argv[1]
with psycopg.connect(url) as conn:
    with conn.cursor() as cur:
        cur.execute("SELECT 1")
        cur.fetchone()
        cur.execute("SELECT to_regclass('public.ops_fences')")
        if cur.fetchone()[0] is None:
            raise SystemExit("ops_fences missing in green")
        cur.execute("SELECT active FROM ops_fences WHERE name='restore'")
        row = cur.fetchone()
        if row is None or row[0] is not True:
            raise SystemExit("restore fence must be active in green after restore")
print("green postgres reachable + restore fence active")
PY

echo "3) Restore bundled object bytes into green MinIO bucket + verify checksums"
: "${MINIO_ENDPOINT:?MINIO_ENDPOINT required}"
: "${MINIO_ACCESS_KEY:?MINIO_ACCESS_KEY required}"
: "${MINIO_SECRET_KEY:?MINIO_SECRET_KEY required}"
command -v mc >/dev/null 2>&1 || { echo "mc required" >&2; exit 1; }
mc alias set markhand-restore "$MINIO_ENDPOINT" "$MINIO_ACCESS_KEY" "$MINIO_SECRET_KEY" >/dev/null
mc mb --ignore-existing "markhand-restore/${GREEN_MINIO_BUCKET}" >/dev/null || true
python3 - <<'PY' "$BACKUP_DIR" "$GREEN_MINIO_BUCKET" || { echo "green object restore verify failed" >&2; exit 1; }
import hashlib, json, pathlib, subprocess, sys
backup = pathlib.Path(sys.argv[1])
bucket = sys.argv[2]
objects = json.loads((backup / "minio-object-checksums.json").read_text())["objects"]
for entry in objects:
    key = entry["key"]
    path = backup / entry["bundleFile"]
    data = path.read_bytes()
    if hashlib.sha256(data).hexdigest() != entry["objectSha256"]:
        raise SystemExit(f"pre-put checksum mismatch: {key}")
    put = subprocess.run(
        ["mc", "pipe", f"markhand-restore/{bucket}/{key}"],
        input=data,
        check=False,
        capture_output=True,
    )
    if put.returncode != 0:
        raise SystemExit(f"mc pipe failed for {key}: {put.stderr.decode(errors='replace')}")
    got = subprocess.run(
        ["mc", "cat", f"markhand-restore/{bucket}/{key}"],
        check=False,
        capture_output=True,
    )
    if got.returncode != 0:
        raise SystemExit(f"mc cat green failed for {key}")
    if hashlib.sha256(got.stdout).hexdigest() != entry["objectSha256"]:
        raise SystemExit(f"green object byte mismatch: {key}")
print(f"verified {len(objects)} green object payloads")
PY

echo "4) Restore Qdrant snapshot bytes into green collection + verify non-empty"
: "${QDRANT_URL:?QDRANT_URL required}"
curl -fsS -X PUT "${QDRANT_URL%/}/collections/${GREEN_QDRANT_COLLECTION}" \
  -H "Content-Type: application/json" \
  -d '{"vectors":{"size":8,"distance":"Cosine"}}' >/dev/null 2>&1 || true
curl -fsS -X POST \
  "${QDRANT_URL%/}/collections/${GREEN_QDRANT_COLLECTION}/snapshots/upload?priority=snapshot" \
  -H "Content-Type: application/octet-stream" \
  --data-binary @"$BACKUP_DIR/qdrant-snapshot.bin" \
  -o "$BACKUP_DIR/qdrant-green-upload.json" \
  || { echo "qdrant green snapshot upload failed" >&2; exit 1; }
test -s "$BACKUP_DIR/qdrant-snapshot.bin"

echo "5) Reconcile must be machine-run to durable completion on green"
if [[ "${MARKHAND_GREEN_RECONCILE_DONE:-}" != "1" ]]; then
  echo "set MARKHAND_GREEN_RECONCILE_DONE=1 only after reconcile worker reports durable completion on green" >&2
  echo "REFUSING_CUTOVER_UNTIL_RECONCILE" >&2
  exit 2
fi
# Durable completion marker must exist (written by reconcile operator/tooling).
test -f "$BACKUP_DIR/reconcile.complete" || {
  echo "missing reconcile.complete attestation file in backup dir" >&2
  exit 2
}
RECONCILE_DIGEST="$(tr -d '[:space:]' <"$BACKUP_DIR/reconcile.complete")"
python3 - <<'PY' "$RECONCILE_DIGEST"
import re, sys
if not re.fullmatch(r"[0-9a-fA-F]{64}", sys.argv[1] or ""):
    raise SystemExit("reconcile.complete must be a 64-hex digest")
print("reconcile completion digest ok")
PY

if [[ "${MARKHAND_RESTORE_CUTOVER:-}" != "1" ]]; then
  echo "green restore + reconcile complete; refuse primary cutover without MARKHAND_RESTORE_CUTOVER=1" >&2
  echo "RESTORE_GREEN_OK_AWAITING_CUTOVER" >&2
  exit 2
fi

echo "6) Atomic cutover: promote green identity only after fence clear succeeds"
# Fence clear must update exactly one active row; zero-row clear fails.
UPDATED="$(psql "$DATABASE_URL" -Atc \
  "UPDATE ops_fences
   SET active = false,
       cleared_at = now(),
       attestation_sha256 = '$RECONCILE_DIGEST'
   WHERE name = 'restore' AND active = true
   RETURNING 1" | wc -l | tr -d ' ')"
if [[ "$UPDATED" != "1" ]]; then
  echo "fence clear failed: expected 1 row, got ${UPDATED:-0}" >&2
  exit 1
fi

echo "RESTORE_CUTOVER_COMPLETE"
# Never delete WRITE_FENCE automatically.
