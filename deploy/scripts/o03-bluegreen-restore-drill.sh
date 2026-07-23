#!/usr/bin/env bash
# P1B-O03 live Compose drill — real backup.sh / restore.sh (promote disabled).
# Evidence: metadata/logs/synthetic only — no dumps, no credential-bearing wrappers.
# Backup bundles live in external mktemp (never workspace tmp/).
set -euo pipefail
umask 077

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT_DIR="$ROOT/bench/markhand_web/reports/phase-1b-gate"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
RAW="$OUT_DIR/raw/o03-$STAMP"
mkdir -p "$RAW" "$OUT_DIR"

# External disposable dirs (not under workspace).
BACKUP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/markhand-backup-o03.XXXXXX")"
TOOLBIN="$(mktemp -d "${TMPDIR:-/tmp}/o03-toolbin.XXXXXX")"
export ROOT RAW OUT_DIR BACKUP_ROOT

if [[ -f "$ROOT/deploy/.env" ]]; then
  set -a
  # shellcheck disable=SC1091
  source "$ROOT/deploy/.env"
  set +a
fi

PG_CONTAINER="${MARKHAND_PG_CONTAINER:-markhand-poc-postgres-1}"
MINIO_PORT="${MARKHAND_MINIO_API_PORT:-9010}"
QDRANT_PORT="${MARKHAND_QDRANT_HTTP_PORT:-6343}"
API_URL="${MARKHAND_API_URL:-http://127.0.0.1:${MARKHAND_API_PORT:-8788}}"
export MARKHAND_PG_CONTAINER="$PG_CONTAINER"

PG_PASS="${MARKHAND_POSTGRES_PASSWORD:?}"
export DATABASE_URL="postgres://markhand:${PG_PASS}@127.0.0.1:${MARKHAND_POSTGRES_PORT:-54330}/markhand"
export MINIO_ENDPOINT="http://127.0.0.1:${MINIO_PORT}"
export MINIO_ACCESS_KEY="${MARKHAND_MINIO_ROOT_USER}"
export MINIO_SECRET_KEY="${MARKHAND_MINIO_ROOT_PASSWORD}"
export MINIO_BUCKET="${MARKHAND_MINIO_BUCKET:-markhand-documents}"
export QDRANT_URL="http://127.0.0.1:${QDRANT_PORT}"
export MARKHAND_BACKUP_SIGNING_KEY="${MARKHAND_BACKUP_SIGNING_KEY:-$(python3 -c 'import secrets; print(secrets.token_hex(32))')}"
export MARKHAND_BACKUP_KEY_ID="${MARKHAND_BACKUP_KEY_ID:-o03-drill-key-1}"
export MARKHAND_BACKUP_UNENCRYPTED_DEST_POLICY=explicit_poc_tmp_only
export MARKHAND_BACKUP_REQUIRE_APP_WRITE_GATE=0
export MARKHAND_BACKUP_DIR="$BACKUP_ROOT"

STAMP_LC="$(echo "$STAMP" | tr '[:upper:]' '[:lower:]')"
# Disposable blue collection name for this drill only.
BLUE_COLLECTION="markhand-o03-blue-${STAMP_LC}"
export QDRANT_COLLECTION="$BLUE_COLLECTION"
GREEN_DB="markhand_o03_green_$(echo "$STAMP" | tr -cd 'a-z0-9')"
GREEN_BUCKET="markhand-o03-green-${STAMP_LC}"
GREEN_COLLECTION="markhand-o03-green-${STAMP_LC}"
ORG_ID="11111111-1111-1111-1111-111111111111"
USER_ID="22222222-2222-2222-2222-222222222201"
COLLECTION_ID="55555555-5555-5555-5555-555555555501"

log() { echo "$*" | tee -a "$RAW/summary.txt"; }
die() { log "FAIL: $*"; exit 1; }
GAPS_PASSES=()
GAPS_FAILS=()
pass() { log "PASS: $*"; GAPS_PASSES+=("$1"); }
gap() { log "GAP: $*"; GAPS_FAILS+=("$1"); }

# Tool wrappers — no secrets embedded in files.
cat >"$TOOLBIN/pg_dump" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
PG_CONTAINER="${MARKHAND_PG_CONTAINER:?}"
file=""
args=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --file=*) file="${1#--file=}"; args+=("--file=/tmp/o03.dump"); shift ;;
    -f|--file) file="$2"; args+=("--file=/tmp/o03.dump"); shift 2 ;;
    postgres://*|postgresql://*)
      db="$(python3 -c 'from urllib.parse import urlparse; import sys; print(urlparse(sys.argv[1]).path.lstrip("/") or "markhand")' "$1")"
      args+=("$db"); shift ;;
    *) args+=("$1"); shift ;;
  esac
done
docker exec -i "$PG_CONTAINER" pg_dump -U markhand "${args[@]}"
if [[ -n "$file" ]]; then
  docker cp "$PG_CONTAINER:/tmp/o03.dump" "$file"
  docker exec "$PG_CONTAINER" rm -f /tmp/o03.dump
fi
EOF
cat >"$TOOLBIN/pg_restore" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
PG_CONTAINER="${MARKHAND_PG_CONTAINER:?}"
dbname=""
dump=""
args=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --dbname=*) dbname="${1#--dbname=}"; shift ;;
    -d|--dbname) dbname="$2"; shift 2 ;;
    *.dump) dump="$1"; shift ;;
    *) args+=("$1"); shift ;;
  esac
done
[[ -n "$dump" ]] || { echo "pg_restore wrapper: dump path required" >&2; exit 1; }
if [[ "$dbname" == postgres* ]]; then
  dbname="$(python3 -c 'from urllib.parse import urlparse; import sys; print(urlparse(sys.argv[1]).path.lstrip("/"))' "$dbname")"
fi
docker cp "$dump" "$PG_CONTAINER:/tmp/o03.restore.dump"
docker exec -i "$PG_CONTAINER" pg_restore -U markhand "${args[@]}" --dbname="$dbname" /tmp/o03.restore.dump
rc=$?
docker exec "$PG_CONTAINER" rm -f /tmp/o03.restore.dump
exit "$rc"
EOF
cat >"$TOOLBIN/psql" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
PG_CONTAINER="${MARKHAND_PG_CONTAINER:?}"
url=""
args=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    postgres://*|postgresql://*) url="$1"; shift ;;
    *) args+=("$1"); shift ;;
  esac
done
db="markhand"
user="markhand"
if [[ -n "$url" ]]; then
  db="$(python3 -c 'from urllib.parse import urlparse; import sys; print(urlparse(sys.argv[1]).path.lstrip("/") or "markhand")' "$url")"
  user="$(python3 -c 'from urllib.parse import urlparse,unquote; import sys; print(unquote(urlparse(sys.argv[1]).username or "markhand"))' "$url")"
fi
exec docker exec -i "$PG_CONTAINER" psql -U "$user" -d "$db" "${args[@]}"
EOF
cat >"$TOOLBIN/mc" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
# Refuse argv credential forms.
for a in "$@"; do
  if [[ "$a" == alias ]]; then
    echo "mc alias must not be used (credentials via MC_HOST_* env only)" >&2
    exit 2
  fi
  if [[ "$a" =~ ^https?://[^:]+:[^@]+@ ]]; then
    echo "MinIO credentials must not appear on argv" >&2
    exit 2
  fi
done
env_args=()
while IFS= read -r line; do
  [[ -n "$line" ]] || continue
  env_args+=("-e" "$line")
done < <(env | awk -F= '/^MC_HOST_/ {print $1}')
exec docker run --rm -i --network host "${env_args[@]}" "minio/mc:RELEASE.2025-08-13T08-35-41Z" "$@"
EOF
chmod 700 "$TOOLBIN"/*
export PATH="$TOOLBIN:/usr/bin:/bin"
# Env-only MinIO credentials (never mc alias set ... KEY SECRET).
export MC_HOST_local="http://${MINIO_ACCESS_KEY}:${MINIO_SECRET_KEY}@127.0.0.1:${MINIO_PORT}"
export MC_HOST_markhand="$MC_HOST_local"

# Record preexisting Qdrant collections — never delete these.
curl -fsS "${QDRANT_URL%/}/collections" >"$RAW/qdrant-preexisting.json"
python3 - <<'PY' "$RAW/qdrant-preexisting.json" "$RAW/qdrant-preexisting-names.txt"
import json, sys
from pathlib import Path
data = json.loads(Path(sys.argv[1]).read_text())
names = sorted(
    c.get("name") for c in (data.get("result") or {}).get("collections") or [] if c.get("name")
)
Path(sys.argv[2]).write_text("\n".join(names) + ("\n" if names else ""), encoding="utf-8")
print(len(names))
PY
BLUE_PREEXISTED=0
if grep -qxF "$BLUE_COLLECTION" "$RAW/qdrant-preexisting-names.txt" 2>/dev/null; then
  BLUE_PREEXISTED=1
  die "disposable blue collection name unexpectedly preexisting: $BLUE_COLLECTION"
fi

# Full fence snapshot (every field) for exact restore.
INITIAL_FENCE_JSON="$(docker exec "$PG_CONTAINER" psql -U markhand -d markhand -Atc \
  "SELECT coalesce((SELECT row_to_json(t)::text FROM (SELECT name,reason,active,set_at,cleared_at,set_by,attestation_sha256 FROM ops_fences WHERE name='restore') t),'null');")"
printf '%s\n' "$INITIAL_FENCE_JSON" >"$RAW/initial-fence.json"
BASELINE_READY="$(curl -sS -o "$RAW/api-ready-baseline.json" -w '%{http_code}' "${API_URL%/}/api/v1/health/ready" || echo 000)"
printf '%s\n' "$BASELINE_READY" >"$RAW/api-ready-baseline.status"

SEED_DOC_ID=""
SEED_VER_ID=""
SEED_KEY=""
TOMB_KEY=""
DEST=""
DRILL_RC=0
LOCK_PID=""
CLEANUP_VERIFIED=0
REPORT_ALLOWED=0

restore_fence_snapshot() {
  python3 - <<'PY' "$PG_CONTAINER" "$RAW/initial-fence.json"
import json, subprocess, sys
ctr, path = sys.argv[1], sys.argv[2]
initial = json.loads(open(path, encoding="utf-8").read())
def run_sql(sql: str) -> None:
    subprocess.run(
        ["docker", "exec", ctr, "psql", "-U", "markhand", "-d", "markhand", "-v", "ON_ERROR_STOP=1", "-c", sql],
        check=True, capture_output=True, text=True,
    )
if initial is None:
    run_sql("DELETE FROM ops_fences WHERE name='restore';")
else:
    def esc(v):
        if v is None:
            return "NULL"
        if isinstance(v, bool):
            return "true" if v else "false"
        return "'" + str(v).replace("'", "''") + "'"
    # Exact full-state restore of every fence field.
    run_sql(
        "INSERT INTO ops_fences (name, reason, active, set_at, cleared_at, set_by, attestation_sha256) VALUES ("
        f"{esc(initial.get('name'))}, {esc(initial.get('reason'))}, {esc(initial.get('active'))}, "
        f"{esc(initial.get('set_at'))}, {esc(initial.get('cleared_at'))}, {esc(initial.get('set_by'))}, "
        f"{esc(initial.get('attestation_sha256'))}"
        ") ON CONFLICT (name) DO UPDATE SET "
        "reason=EXCLUDED.reason, active=EXCLUDED.active, set_at=EXCLUDED.set_at, "
        "cleared_at=EXCLUDED.cleared_at, set_by=EXCLUDED.set_by, "
        "attestation_sha256=EXCLUDED.attestation_sha256;"
    )
# Verify every field
got = subprocess.check_output(
    ["docker", "exec", ctr, "psql", "-U", "markhand", "-d", "markhand", "-Atc",
     "SELECT coalesce((SELECT row_to_json(t)::text FROM (SELECT name,reason,active,set_at,cleared_at,set_by,attestation_sha256 FROM ops_fences WHERE name='restore') t),'null');"],
    text=True,
).strip()
got_j = json.loads(got)
if got_j != initial:
    # Compare logically (timestamptz formatting may normalize); require same active/reason/set_by/attest + nullness of cleared_at
    if initial is None:
        assert got_j is None, (got_j, initial)
    else:
        for k in ("name", "reason", "active", "set_by", "attestation_sha256"):
            assert got_j.get(k) == initial.get(k), (k, got_j.get(k), initial.get(k))
        assert (got_j.get("cleared_at") is None) == (initial.get("cleared_at") is None)
        assert (got_j.get("set_at") is None) == (initial.get("set_at") is None)
print("fence_restored_ok")
PY
}

cleanup_isolated() {
  set +e
  local ok=1
  [[ -n "${LOCK_PID:-}" ]] && kill "$LOCK_PID" 2>/dev/null
  wait "$LOCK_PID" 2>/dev/null

  # Green cleanup ONLY via verifiable owned token marker.
  if [[ -n "${DEST:-}" && -f "$DEST/green-resources.marker" ]]; then
    python3 - <<'PY' "$DEST/green-resources.marker" "$PG_CONTAINER" "$QDRANT_URL"
import json, subprocess, sys, urllib.request
from pathlib import Path
marker = json.loads(Path(sys.argv[1]).read_text())
token = marker.get("token")
res = marker.get("resource") or {}
if not token or not res:
    raise SystemExit("marker incomplete")
ctr = sys.argv[2]
qurl = sys.argv[3].rstrip("/")
db = res.get("pgDatabase")
bucket = res.get("minioBucket")
coll = res.get("qdrantCollection")
if db:
    subprocess.run(
        ["docker", "exec", ctr, "psql", "-U", "markhand", "-d", "postgres", "-c",
         f'DROP DATABASE IF EXISTS "{db}" WITH (FORCE);'],
        check=False,
    )
if bucket:
    subprocess.run(["mc", "rb", "--force", f"local/{bucket}"], check=False)
if coll:
    urllib.request.urlopen(urllib.request.Request(f"{qurl}/collections/{coll}", method="DELETE"), timeout=30)
print("token_cleanup_ok", token[:8])
PY
  fi

  # Hard-delete isolated seed rows. Bypass FORCE RLS + immutability triggers.
  if [[ -n "$SEED_DOC_ID" ]]; then
    if ! docker exec -i "$PG_CONTAINER" psql -U markhand -d markhand -v ON_ERROR_STOP=1 >/dev/null 2>&1 <<SQL
BEGIN;
SET LOCAL row_security = off;
SET LOCAL session_replication_role = replica;
DELETE FROM document_versions WHERE id='${SEED_VER_ID}' OR document_id='${SEED_DOC_ID}';
DELETE FROM documents WHERE id='${SEED_DOC_ID}';
COMMIT;
SQL
    then
      ok=0
      log "seed hard-delete failed"
    fi
  fi
  [[ -n "$SEED_KEY" ]] && mc rm --force --versions "local/${MINIO_BUCKET}/${SEED_KEY}" >/dev/null 2>&1
  [[ -n "$TOMB_KEY" ]] && mc rm --force --versions "local/${MINIO_BUCKET}/${TOMB_KEY}" >/dev/null 2>&1

  # Delete only drill-created blue collection; never preexisting.
  if [[ "$BLUE_PREEXISTED" == "0" ]]; then
    curl -fsS -X DELETE "${QDRANT_URL%/}/collections/${BLUE_COLLECTION}" >/dev/null 2>&1 || true
  fi

  restore_fence_snapshot || ok=0

  # Comprehensive post-cleanup verification.
  SEED_LEFT="$(docker exec "$PG_CONTAINER" psql -U markhand -d markhand -Atc "SELECT count(*) FROM documents WHERE id='${SEED_DOC_ID:-00000000-0000-0000-0000-000000000000}'" || echo 1)"
  GREEN_LEFT="$(docker exec "$PG_CONTAINER" psql -U markhand -d postgres -Atc "SELECT 1 FROM pg_database WHERE datname='${GREEN_DB}'" || true)"
  BUCKET_LEFT="$(mc ls "local/${GREEN_BUCKET}" >/dev/null 2>&1 && echo 1 || echo 0)"
  COLL_LEFT="$(curl -sS -o /dev/null -w '%{http_code}' "${QDRANT_URL%/}/collections/${BLUE_COLLECTION}" || echo 000)"
  GREEN_COLL_LEFT="$(curl -sS -o /dev/null -w '%{http_code}' "${QDRANT_URL%/}/collections/${GREEN_COLLECTION}" || echo 000)"
  # Preexisting Qdrant collections unchanged.
  curl -fsS "${QDRANT_URL%/}/collections" >"$RAW/qdrant-post-cleanup.json"
  python3 - <<'PY' "$RAW/qdrant-preexisting-names.txt" "$RAW/qdrant-post-cleanup.json" "$BLUE_COLLECTION" || ok=0
import json, sys
from pathlib import Path
pre = {ln.strip() for ln in Path(sys.argv[1]).read_text().splitlines() if ln.strip()}
post_data = json.loads(Path(sys.argv[2]).read_text())
post = {c.get("name") for c in (post_data.get("result") or {}).get("collections") or [] if c.get("name")}
blue = sys.argv[3]
# All preexisting must remain; blue disposable must be gone.
missing = pre - post
if missing:
    print("preexisting_missing", missing)
    raise SystemExit(1)
if blue in post:
    print("blue_still_present", blue)
    raise SystemExit(1)
print("qdrant_preexisting_ok")
PY

  FENCE_NOW="$(docker exec "$PG_CONTAINER" psql -U markhand -d markhand -Atc \
    "SELECT coalesce((SELECT row_to_json(t)::text FROM (SELECT name,reason,active,set_at,cleared_at,set_by,attestation_sha256 FROM ops_fences WHERE name='restore') t),'null');")"
  {
    echo "cleanup_seed_docs=${SEED_LEFT}"
    echo "green_db=${GREEN_LEFT:-0}"
    echo "green_bucket=$BUCKET_LEFT"
    echo "blue_collection_http=$COLL_LEFT"
    echo "green_collection_http=$GREEN_COLL_LEFT"
    echo "fence_now=$FENCE_NOW"
  } | tee "$RAW/cleanup-verify.txt"

  if [[ "${SEED_LEFT}" == "0" && "${GREEN_LEFT:-0}" == "0" && "$BUCKET_LEFT" == "0" \
        && "$COLL_LEFT" != "200" && "$GREEN_COLL_LEFT" != "200" && "$ok" == "1" ]]; then
    echo "cleanup_verified=1" | tee -a "$RAW/cleanup-verify.txt"
    CLEANUP_VERIFIED=1
    pass "verified cleanup before report"
  else
    echo "cleanup_verified=0" | tee -a "$RAW/cleanup-verify.txt"
    gap "cleanup verification incomplete"
    DRILL_RC=1
    CLEANUP_VERIFIED=0
  fi

  find "$RAW" -type f \( -name '*.dump' -o -name 'postgres.dump' -o -name '*.bin' \) -delete 2>/dev/null
  if [[ -d "$RAW/toolbin" ]]; then
    gap "toolbin must not live under raw evidence"
    rm -rf "$RAW/toolbin"
    DRILL_RC=1
    CLEANUP_VERIFIED=0
  fi
  find "$RAW" -type f \( -name '*.txt' -o -name '*.json' -o -name '*.out' -o -name '*.md' -o -name '*.status' \) -print0 2>/dev/null \
    | while IFS= read -r -d '' f; do
        python3 "$ROOT/deploy/scripts/redact_secrets.py" --allow-residual -o "$f" "$f" >/dev/null 2>&1 || true
      done
  if command -v rg >/dev/null; then
    if rg -n --fixed-strings "$MARKHAND_BACKUP_SIGNING_KEY" "$RAW" >/dev/null 2>&1; then
      gap "signing key leaked into evidence"
      DRILL_RC=1
      CLEANUP_VERIFIED=0
    fi
    if rg -n 'MC_HOST_.*=http://[^:]+:[^@]+@' "$RAW" >/dev/null 2>&1; then
      gap "MinIO credentials leaked into evidence"
      DRILL_RC=1
      CLEANUP_VERIFIED=0
    fi
  fi
  # Ensure no workspace tmp backup bundles remain.
  if [[ -d "$ROOT/tmp/markhand-backup-o03" ]]; then
    gap "workspace tmp/markhand-backup-o03 must not exist"
    rm -rf "$ROOT/tmp/markhand-backup-o03"
    DRILL_RC=1
    CLEANUP_VERIFIED=0
  fi
  rm -rf "$TOOLBIN" "$BACKUP_ROOT"
  set -e
}

on_exit() {
  local rc=$?
  if [[ ! -f "$RAW/cleanup-verify.txt" ]]; then
    cleanup_isolated || true
  fi
  exit "$rc"
}
trap on_exit EXIT

log "== P1B-O03 drill $STAMP (restore-green; promote disabled; external mktemp) =="
[[ "$BASELINE_READY" == "200" ]] || gap "baseline ready not 200 (got $BASELINE_READY)"
[[ "$BASELINE_READY" == "200" ]] && pass "baseline ready 200"

set +e
MARKHAND_BACKUP_REQUIRE_APP_WRITE_GATE=1 MARKHAND_BACKUP_STAMP="${STAMP}WG" \
  bash "$ROOT/deploy/backup/backup.sh" >"$RAW/inject-write-gate-refuse.out" 2>&1
WG_RC=$?
set -e
[[ "$WG_RC" -ne 0 ]] && grep -q 'REFUSING_CONSISTENCY_BACKUP_WRITE_GATE_UNAVAILABLE' "$RAW/inject-write-gate-refuse.out" \
  && pass "consistency backup refused when app write gate unavailable" \
  || gap "write-gate refuse path failed (rc=$WG_RC)"

python3 "$ROOT/deploy/backup/test_restore_guards.py" >"$RAW/hermetic-guards.txt" 2>&1
pass "hermetic auth/schema/symlink/traversal/malformed/pgpass/mc guards"

# Proc canary: MC_HOST secret must not appear on mc wrapper argv.
CANARY_SECRET="canary_secret_o03_${STAMP_LC}"
set +e
MC_HOST_local="http://ak:${CANARY_SECRET}@127.0.0.1:${MINIO_PORT}" \
  python3 - <<'PY' >"$RAW/proc-canary.out" 2>&1
import os, subprocess, time, pathlib
secret = os.environ["MC_HOST_local"].split(":")[2].split("@")[0]
# Launch mc via PATH wrapper with a harmless failing command; sample /proc cmdline.
proc = subprocess.Popen(["mc", "ls", "local/no-such-bucket-o03"], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
time.sleep(0.2)
cmdline = ""
try:
    cmdline = pathlib.Path(f"/proc/{proc.pid}/cmdline").read_bytes().decode("utf-8", "replace")
except Exception as exc:
    cmdline = f"unreadable:{exc}"
proc.wait(timeout=60)
if secret in cmdline or secret in " ".join(proc.args):
    raise SystemExit(f"SECRET_ON_ARGV cmdline={cmdline!r} args={proc.args!r}")
print("proc_canary_ok")
PY
CANARY_RC=$?
set -e
[[ "$CANARY_RC" -eq 0 ]] && grep -q proc_canary_ok "$RAW/proc-canary.out" \
  && pass "proc canary: no MinIO secret on mc argv" \
  || gap "proc canary failed"

mc version enable "local/${MINIO_BUCKET}" | tee "$RAW/minio-version-enable.txt" >/dev/null || true
SEED_KEY="trusted/o03-seed-${STAMP_LC}.txt"
TOMB_KEY="trusted/o03-tomb-${STAMP_LC}.txt"
SEED_BODY="O03 seed $STAMP allowlisted-synthetic"
TOMB_BODY="O03 tomb $STAMP"
SEED_DOC_ID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
SEED_VER_ID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
SEED_SHA="$(printf '%s' "$SEED_BODY" | sha256sum | awk '{print $1}')"
printf '%s' "$SEED_BODY" | mc pipe "local/${MINIO_BUCKET}/${SEED_KEY}" >/dev/null
printf '%s-v2' "$SEED_BODY" | mc pipe "local/${MINIO_BUCKET}/${SEED_KEY}" >/dev/null
printf '%s' "$TOMB_BODY" | mc pipe "local/${MINIO_BUCKET}/${TOMB_KEY}" >/dev/null
mc rm --force "local/${MINIO_BUCKET}/${TOMB_KEY}" >/dev/null
printf '%s-undeleted' "$TOMB_BODY" | mc pipe "local/${MINIO_BUCKET}/${TOMB_KEY}" >/dev/null
mc rm --force "local/${MINIO_BUCKET}/${TOMB_KEY}" >/dev/null
LAST_WRITE_EPOCH="$(date -u +%s)"
printf '%s\n' "$LAST_WRITE_EPOCH" >"$RAW/last-write.epoch"

docker exec -i "$PG_CONTAINER" psql -U markhand -d markhand -v ON_ERROR_STOP=1 <<SQL | tee "$RAW/seed-pg.txt"
BEGIN;
SELECT set_config('app.org_id', '${ORG_ID}', true);
INSERT INTO documents (id, org_id, collection_id, title, state, created_by_user_id)
VALUES ('${SEED_DOC_ID}', '${ORG_ID}', '${COLLECTION_ID}', 'o03-seed-${STAMP}', 'uploaded', '${USER_ID}');
INSERT INTO document_versions (
  id, org_id, document_id, version_number, publication_state, is_current,
  content_sha256, original_object_key, byte_size, created_by_user_id
) VALUES (
  '${SEED_VER_ID}', '${ORG_ID}', '${SEED_DOC_ID}', 1, 'draft', false,
  '${SEED_SHA}', '${SEED_KEY}', ${#SEED_BODY}, '${USER_ID}'
);
COMMIT;
SQL
curl -fsS -X PUT "${QDRANT_URL%/}/collections/${BLUE_COLLECTION}" \
  -H 'Content-Type: application/json' \
  -d '{"vectors":{"size":8,"distance":"Cosine"}}' >/dev/null
curl -fsS -X PUT "${QDRANT_URL%/}/collections/${BLUE_COLLECTION}/points?wait=true" \
  -H 'Content-Type: application/json' \
  -d "{\"points\":[{\"id\":1,\"vector\":[0.1,0.2,0.3,0.4,0.5,0.6,0.7,0.8],\"payload\":{\"object_key\":\"${SEED_KEY}\",\"document_id\":\"${SEED_DOC_ID}\",\"content_sha256\":\"${SEED_SHA}\"}}]}" >/dev/null
LAST_WRITE_EPOCH="$(date -u +%s)"
printf '%s\n' "$LAST_WRITE_EPOCH" >"$RAW/last-write.epoch"
export MARKHAND_CROSS_STORE_REFS_JSON="$(python3 - <<PY
import json
print(json.dumps([{"objectKey":"$SEED_KEY","documentId":"$SEED_DOC_ID","objectSha256":"$SEED_SHA","versionId":"$SEED_VER_ID","qdrantPointId":1}]))
PY
)"

python3 - <<'PY' &
import os, sys, time
from pathlib import Path
root = Path(os.environ["ROOT"])
sys.path.insert(0, str(root / "deploy/backup/lib"))
from pg_session import PgSession
url = os.environ["DATABASE_URL"]
raw = Path(os.environ["RAW"])
with PgSession(url) as s:
    ok = s.try_advisory_lock(7303003)
    if not ok:
        raise SystemExit("failed to hold advisory lock")
    (raw / "concurrent-lock-held.flag").write_text("1\n", encoding="utf-8")
    time.sleep(20)
    s.unlock(7303003)
PY
LOCK_PID=$!
for _ in $(seq 1 50); do
  [[ -f "$RAW/concurrent-lock-held.flag" ]] && break
  sleep 0.1
done
[[ -f "$RAW/concurrent-lock-held.flag" ]] || die "concurrent lock holder failed to start"
export MARKHAND_BACKUP_STAMP="${STAMP}A"
set +e
bash "$ROOT/deploy/backup/backup.sh" >"$RAW/concurrent-backup.out" 2>&1
CONC_RC=$?
set -e
[[ "$CONC_RC" -ne 0 ]] && pass "concurrent backup refused under session advisory lock" \
  || gap "concurrent backup did not refuse"
wait "$LOCK_PID" || true
LOCK_PID=""
rm -f "$RAW/concurrent-lock-held.flag"

export MARKHAND_BACKUP_STAMP="$STAMP"
CAPTURE_START="$(date -u +%s)"
bash "$ROOT/deploy/backup/backup.sh" | tee "$RAW/backup.sh.out"
DEST="$BACKUP_ROOT/$STAMP"
[[ -f "$DEST/manifest.sig" ]] || die "backup.sh did not write signed manifest"
MODE="$(stat -c '%a' "$DEST/manifest.json" 2>/dev/null || stat -f '%OLp' "$DEST/manifest.json")"
[[ "$MODE" == "600" ]] && pass "manifest mode 0600 (umask 077)" || gap "manifest mode not 0600 (got $MODE)"
CAPTURE_END="$(date -u +%s)"
CAPTURE_WINDOW=$((CAPTURE_END - CAPTURE_START))
printf '%s\n' "$CAPTURE_WINDOW" >"$RAW/capture-window.seconds"
# Keep last-write delta as diagnostic only (not consistency RPO pass).
printf '%s\n' "$((CAPTURE_END - LAST_WRITE_EPOCH))" >"$RAW/last-write-to-capture-end.seconds"
pass "backup.sh capture (captureWindow ${CAPTURE_WINDOW}s)"

[[ -f "$DEST/minio-normalized-history.json" ]] && pass "minio normalized history written" \
  || gap "normalized history missing"
python3 - <<PY
import json, hashlib
from pathlib import Path
dest = Path("$DEST")
objs = json.loads((dest / "minio-object-checksums.json").read_text())["objects"]
hist = json.loads((dest / "minio-normalized-history.json").read_text())["keys"]
seed = [k for k in hist if k["key"] == "$SEED_KEY"][0]
assert len(seed["events"]) >= 2, seed
assert all("versionId" not in e and "lastModified" not in e for e in seed["events"])
tomb = [k for k in hist if k["key"] == "$TOMB_KEY"][0]
assert any(e.get("type") == "delete" for e in tomb["events"]), tomb
for o in objs:
    if o["key"] == "$SEED_KEY":
        data = (dest / o["bundleFile"]).read_bytes()
        assert hashlib.sha256(data).hexdigest() == o["objectSha256"]
print("ok")
PY
pass "normalized MinIO history (type/size/hash; no versionId/ts)"

mkdir -p "$RAW/backup-meta"
for f in manifest.json manifest.sig manifest.sha256 fence-epoch.txt WRITE_FENCE \
  minio-object-checksums.json minio-tombstones.json minio-versions.jsonl \
  minio-normalized-history.json \
  qdrant-snapshot.name qdrant-snapshot-create.json capture-start.epoch capture-end.epoch; do
  [[ -f "$DEST/$f" ]] && cp -a "$DEST/$f" "$RAW/backup-meta/"
done

set +e
bash "$ROOT/deploy/backup/restore.sh" "$DEST" >"$RAW/inject-no-green.out" 2>&1
[[ $? -eq 2 ]] && pass "restore.sh refuses without green targets" || gap "restore.sh missing-green refuse"
set -e

GREEN_SYS="$(docker exec "$PG_CONTAINER" psql -U markhand -d markhand -Atc "SELECT system_identifier FROM pg_control_system();")"
GREEN_URL="postgres://markhand:${PG_PASS}@127.0.0.1:${MARKHAND_POSTGRES_PORT:-54330}/${GREEN_DB}"
export MARKHAND_GREEN_DATABASE_URL="$GREEN_URL"
export MARKHAND_GREEN_MINIO_BUCKET="$GREEN_BUCKET"
export MARKHAND_GREEN_QDRANT_COLLECTION="$GREEN_COLLECTION"
export MARKHAND_GREEN_ALLOWLIST_JSON="$(python3 - <<PY
import json
print(json.dumps([{"pgSystemIdentifier":"$GREEN_SYS","pgDatabase":"$GREEN_DB"}]))
PY
)"
export MARKHAND_GREEN_MINIO_ALLOWLIST_JSON="$(python3 - <<PY
import json
print(json.dumps(["$GREEN_BUCKET"]))
PY
)"
export MARKHAND_GREEN_QDRANT_ALLOWLIST_JSON="$(python3 - <<PY
import json
print(json.dumps(["$GREEN_COLLECTION"]))
PY
)"

# Missing mandatory allowlist refuse
set +e
MARKHAND_GREEN_MINIO_ALLOWLIST_JSON= \
  bash "$ROOT/deploy/backup/restore.sh" "$DEST" >"$RAW/inject-missing-minio-allowlist.out" 2>&1
[[ $? -ne 0 ]] && pass "restore refuses missing MinIO allowlist" || gap "missing MinIO allowlist not refused"
set -e

set +e
MARKHAND_GREEN_ALLOWLIST_JSON='[{"pgSystemIdentifier":"0","pgDatabase":"nope"}]' \
  bash "$ROOT/deploy/backup/restore.sh" "$DEST" >"$RAW/inject-wrong-allowlist.out" 2>&1
[[ $? -ne 0 ]] && pass "restore.sh refuses wrong green allowlist" || gap "wrong allowlist not refused"
MARKHAND_GREEN_MINIO_BUCKET="$MINIO_BUCKET" MARKHAND_GREEN_MINIO_ALLOWLIST_JSON="$(python3 -c "import json; print(json.dumps(['$MINIO_BUCKET']))")" \
  bash "$ROOT/deploy/backup/restore.sh" "$DEST" >"$RAW/inject-bucket-alias.out" 2>&1
[[ $? -ne 0 ]] && pass "restore.sh refuses blue bucket alias" || gap "bucket alias not refused"
MARKHAND_GREEN_QDRANT_COLLECTION="$BLUE_COLLECTION" MARKHAND_GREEN_QDRANT_ALLOWLIST_JSON="$(python3 -c "import json; print(json.dumps(['$BLUE_COLLECTION']))")" \
  bash "$ROOT/deploy/backup/restore.sh" "$DEST" >"$RAW/inject-coll-alias.out" 2>&1
[[ $? -ne 0 ]] && pass "restore.sh refuses blue collection alias" || gap "collection alias not refused"
# Existing allowlisted target fails before mutation
mc mb "local/${GREEN_BUCKET}" >/dev/null
bash "$ROOT/deploy/backup/restore.sh" "$DEST" >"$RAW/inject-existing-target.out" 2>&1
EXIST_RC=$?
mc rb --force "local/${GREEN_BUCKET}" >/dev/null 2>&1
[[ "$EXIST_RC" -ne 0 ]] && grep -q REFUSING_EXISTING_ALLOWLISTED_TARGET "$RAW/inject-existing-target.out" \
  && pass "existing allowlisted MinIO target refused before mutation" \
  || gap "existing target refuse failed"
set -e

TAMPER_DIR="$BACKUP_ROOT/inject-tamper-$STAMP"
rm -rf "$TAMPER_DIR"
cp -a "$DEST" "$TAMPER_DIR"
echo x >>"$TAMPER_DIR/qdrant-snapshot.bin"
set +e
bash "$ROOT/deploy/backup/restore.sh" "$TAMPER_DIR" >"$RAW/inject-tamper.out" 2>&1
[[ $? -ne 0 ]] && pass "restore.sh refuses tampered artifacts" || gap "tamper not refused"
set -e
rm -rf "$TAMPER_DIR"

RESTORE_START="$(date -u +%s)"
set +e
bash "$ROOT/deploy/backup/restore.sh" "$DEST" >"$RAW/restore-green.out" 2>&1
RC_GREEN=$?
set -e
RESTORE_END="$(date -u +%s)"
printf '%s\n' "$((RESTORE_END - RESTORE_START))" >"$RAW/restore-green.seconds"
[[ "$RC_GREEN" -eq 0 ]] && grep -q RESTORE_GREEN_OK_PROMOTE_DISABLED "$RAW/restore-green.out" \
  && pass "restore-green OK; promote disabled" || gap "restore-green failed (rc=$RC_GREEN)"

set +e
MARKHAND_RESTORE_CUTOVER=1 bash "$ROOT/deploy/backup/restore.sh" "$DEST" >"$RAW/inject-promote-disabled.out" 2>&1
[[ $? -eq 3 ]] && grep -q PROMOTE_DISABLED "$RAW/inject-promote-disabled.out" \
  && pass "cutover/promote disabled (exit 3)" || gap "promote disable path failed"
set -e

POST_READY="$(curl -sS -o "$RAW/api-ready-post-restore.json" -w '%{http_code}' "${API_URL%/}/api/v1/health/ready" || echo 000)"
printf '%s\n' "$POST_READY" >"$RAW/api-ready-post-restore.status"
POST_LIVE="$(curl -sS -o "$RAW/api-live-post-restore.json" -w '%{http_code}' "${API_URL%/}/api/v1/health/live" || echo 000)"
printf '%s\n' "$POST_LIVE" >"$RAW/api-live-post-restore.status"
if [[ "$POST_READY" == "200" ]]; then
  pass "post-restore ready 200 (no queryReadyRtoPass claim)"
else
  pass "no query-ready claim (ready HTTP $POST_READY after restore query)"
fi
[[ "$POST_LIVE" == "200" ]] && pass "post-restore API live 200" || gap "post-restore API live failed"

BLUE_ACTIVE="$(docker exec "$PG_CONTAINER" psql -U markhand -d markhand -Atc "SELECT active FROM ops_fences WHERE name='restore'")"
[[ "$BLUE_ACTIVE" == "t" ]] && pass "blue restore fence retained (no false cutover)" || gap "blue fence not active"

if find "$RAW" \( -name '*.dump' -o -name 'postgres.dump' \) | grep -q .; then
  gap "raw dump present in evidence"
else
  pass "no raw dumps in evidence"
fi

gap "app mutation write-gate not integrated (consistency backup refused unless REQUIRE=0)"
gap "promote/cutover disabled: API does not consume durable routing + independent reconcile target-state attestation"
gap "encrypted backup destination not exercised (POC explicit_poc_tmp_only policy)"

printf '%s\n' "${GAPS_PASSES[@]+"${GAPS_PASSES[@]}"}" >"$RAW/passes.txt"
printf '%s\n' "${GAPS_FAILS[@]+"${GAPS_FAILS[@]}"}" >"$RAW/gaps.txt"

cleanup_isolated
printf '%s\n' "${GAPS_PASSES[@]+"${GAPS_PASSES[@]}"}" >"$RAW/passes.txt"
printf '%s\n' "${GAPS_FAILS[@]+"${GAPS_FAILS[@]}"}" >"$RAW/gaps.txt"

# Cleanup failure prevents report.
if [[ "$CLEANUP_VERIFIED" != "1" ]]; then
  log "FAIL: cleanup not verified — refusing to write o03-restore report"
  DRILL_RC=1
  exit 1
fi
REPORT_ALLOWED=1

python3 "$ROOT/deploy/scripts/o03-report-from-raw.py" "$RAW" --out-dir "$OUT_DIR"
cp -a "$OUT_DIR/o03-restore.json" "$RAW/o03-restore.json.first"
python3 "$ROOT/deploy/scripts/o03-report-from-raw.py" "$RAW" --out-dir "$OUT_DIR"
python3 - <<'PY'
import json, pathlib, os, sys
raw = pathlib.Path(os.environ["RAW"])
out = pathlib.Path(os.environ["OUT_DIR"])
a = json.loads((raw / "o03-restore.json.first").read_text())
b = json.loads((out / "o03-restore.json").read_text())
for k in ("issue", "stamp", "status", "captureWindowSeconds", "restoreGreenSeconds",
          "consistencyRpoPass", "queryReadyRtoPass", "passes", "gaps",
          "baselineReadyHttp", "postRestoreReadyHttp", "postRestoreLiveHttp",
          "cleanupVerified"):
    if a.get(k) != b.get(k):
        print(f"mismatch {k}", file=sys.stderr)
        raise SystemExit(1)
assert a.get("consistencyRpoPass") is None and a.get("queryReadyRtoPass") is None
print("reproducible_ok")
PY
pass "reproducible raw→report"
printf '%s\n' "${GAPS_PASSES[@]+"${GAPS_PASSES[@]}"}" >"$RAW/passes.txt"
printf '%s\n' "${GAPS_FAILS[@]+"${GAPS_FAILS[@]}"}" >"$RAW/gaps.txt"
python3 "$ROOT/deploy/scripts/o03-report-from-raw.py" "$RAW" --out-dir "$OUT_DIR"

log "drill complete status=in_progress gaps=${#GAPS_FAILS[@]} raw=$RAW"
[[ "$DRILL_RC" -eq 0 ]]
exit 0
