#!/usr/bin/env bash
# Collect P1B-F02 Docker runtime boot / isolation / sandbox-preflight evidence.
# Requires a healthy POC stack from deploy/scripts/poc-up.sh.
#
# Hermetic validator (no Docker / no secrets from deploy/.env):
#   deploy/scripts/poc-boot-evidence.sh --self-test
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

if [[ "${1:-}" == "--self-test" ]]; then
  exec python3 "$ROOT/deploy/scripts/poc_f02_boot_evidence.py" --self-test
fi

# shellcheck source=poc-compose.sh
source "$ROOT/deploy/scripts/poc-compose.sh"
poc_compose_init

OUT_DIR="${1:-$ROOT/bench/markhand_web/reports}"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
RAW_DIR="${POC_EVIDENCE_RAW_DIR:-/tmp/markhand-f02-evidence-$STAMP}"
REPORT="$OUT_DIR/poc-f02-boot.md"
JSON="$OUT_DIR/poc-f02-boot.json"
FAIL=0
NOLIMIT=0
if [[ -n "${POC_COMPOSE_EFFECTIVE:-}" ]]; then
  NOLIMIT=1
fi

# Pinned alpine already used by POC mock-embedding (images.lock.json).
EGRESS_PROBE_IMAGE="${POC_EGRESS_PROBE_IMAGE:-python:3.12.12-alpine@sha256:2d91681153dd4b8cdb52d4fd34a17b9edbafa4dd3086143cfd4b6c3a84c1acb0}"

mkdir -p "$OUT_DIR" "$RAW_DIR"

pass() { echo "PASS: $*"; echo "PASS: $*" >>"$RAW_DIR/summary.txt"; }
fail() { echo "FAIL: $*" >&2; echo "FAIL: $*" >>"$RAW_DIR/summary.txt"; FAIL=1; }
note() { echo "NOTE: $*"; echo "NOTE: $*" >>"$RAW_DIR/summary.txt"; }

require_cmd() {
  if command -v "$1" >/dev/null 2>&1; then
    pass "command $1"
  else
    fail "missing command $1"
  fi
}

# Write allowlisted inspect JSON only (never Config.Env / secret-bearing fields).
write_sanitized_inspect() {
  local service="$1"
  local id="$2"
  docker inspect "$id" | python3 -c '
import json, sys
from pathlib import Path
sys.path.insert(0, "'"$ROOT"'/deploy/scripts")
import poc_f02_boot_evidence as f02
raw = json.load(sys.stdin)
Path(sys.argv[1]).write_text(json.dumps(f02.sanitize_inspect(raw), indent=2) + "\n", encoding="utf-8")
' "$RAW_DIR/inspect-$service.json"
}

service_id() {
  local service="$1"
  "${COMPOSE[@]}" ps -q "$service" 2>/dev/null || true
}

collect_service_meta() {
  local service="$1"
  local id image_id repo_json mem nano pids cpu_quota cpu_period
  id="$(service_id "$service")"
  if [[ -z "$id" ]]; then
    fail "service $service not running"
    return 1
  fi
  write_sanitized_inspect "$service" "$id"
  image_id="$(docker inspect --format '{{.Image}}' "$id")"
  repo_json="$(docker image inspect --format '{{json .RepoDigests}}' "$image_id" 2>/dev/null || echo '[]')"
  mem="$(docker inspect --format '{{.HostConfig.Memory}}' "$id")"
  nano="$(docker inspect --format '{{.HostConfig.NanoCpus}}' "$id")"
  pids="$(docker inspect --format '{{.HostConfig.PidsLimit}}' "$id")"
  cpu_quota="$(docker inspect --format '{{.HostConfig.CpuQuota}}' "$id")"
  cpu_period="$(docker inspect --format '{{.HostConfig.CpuPeriod}}' "$id")"
  python3 - "$RAW_DIR/meta.json" "$service" "$id" "$image_id" "$repo_json" "$mem" "$nano" "$pids" "$cpu_quota" "$cpu_period" <<'PY'
import json, pathlib, sys
path, service, cid, image_id, repo_json, mem, nano, pids, quota, period = sys.argv[1:]
meta = {}
p = pathlib.Path(path)
if p.is_file():
    meta = json.loads(p.read_text(encoding="utf-8"))
meta.setdefault("containerIds", {})[service] = cid
meta.setdefault("imageIds", {})[service] = image_id
try:
    repos = json.loads(repo_json) if repo_json else []
except json.JSONDecodeError:
    repos = []
real = [d for d in repos if isinstance(d, str) and "@sha256:" in d]
if real:
    meta.setdefault("imageDigests", {})[service] = real[0]
def num(v):
    if v in ("", "<nil>", "None", "null"):
        return 0
    try:
        return int(v)
    except ValueError:
        try:
            return int(float(v))
        except ValueError:
            return 0
meta.setdefault("resourceLimits", {})[service] = {
    "memory": num(mem),
    "nanoCpus": num(nano),
    "pidsLimit": num(pids),
    "cpuQuota": num(quota),
    "cpuPeriod": num(period),
}
p.write_text(json.dumps(meta, indent=2) + "\n", encoding="utf-8")
PY
  echo "$id"
}

echo "== P1B-F02 Docker boot evidence ==" | tee "$RAW_DIR/summary.txt"
echo "stamp=$STAMP" | tee -a "$RAW_DIR/summary.txt"
echo "compose_profiles=$COMPOSE_PROFILES" | tee -a "$RAW_DIR/summary.txt"
echo "compose_project=${MARKHAND_COMPOSE_PROJECT:-markhand-poc}" | tee -a "$RAW_DIR/summary.txt"
date -u +%Y-%m-%dT%H:%M:%SZ | tee -a "$RAW_DIR/summary.txt"
docker version >"$RAW_DIR/docker-version.txt" 2>&1 || true
docker info >"$RAW_DIR/docker-info.txt" 2>&1 || true

# Persist storage driver + seed meta.json (never log deploy/.env values).
STORAGE_DRIVER="$(docker info --format '{{.Driver}}' 2>/dev/null || echo unknown)"
python3 - "$RAW_DIR/meta.json" "$STORAGE_DRIVER" "${MARKHAND_COMPOSE_PROJECT:-markhand-poc}" "$NOLIMIT" <<'PY'
import json, pathlib, sys
path, driver, project, nolimit = sys.argv[1:]
meta = {
    "storageDriver": driver,
    "composeProject": project,
    "nolimitComposeUsed": nolimit == "1",
    "containerIds": {},
    "imageIds": {},
    "imageDigests": {},
    "resourceLimits": {},
    "egressProbe": {"executed": False},
}
pathlib.Path(path).write_text(json.dumps(meta, indent=2) + "\n", encoding="utf-8")
PY
note "storageDriver=$STORAGE_DRIVER"
if [[ "$NOLIMIT" -eq 1 ]]; then
  note "nolimit compose fallback active — cannot qualify F02 Done"
fi

require_cmd docker
require_cmd curl

"$ROOT/deploy/scripts/poc-health.sh" | tee "$RAW_DIR/poc-health.txt"
pass "poc-health"

# --- Metadata for expected O04 services + limit surfaces ---
for svc in api minio postgres qdrant worker-convert worker-index worker-embedding; do
  collect_service_meta "$svc" || true
done

# --- Isolation: UID / read-only / caps / no-new-privileges / nonzero limits ---
for svc in api worker-convert worker-index worker-embedding; do
  id="$(service_id "$svc")"
  if [[ -z "$id" ]]; then
    continue
  fi
  uid="$(docker inspect --format '{{.Config.User}}' "$id")"
  readonly_root="$(docker inspect --format '{{.HostConfig.ReadonlyRootfs}}' "$id")"
  caps="$(docker inspect --format '{{json .HostConfig.CapDrop}}' "$id")"
  mem="$(docker inspect --format '{{.HostConfig.Memory}}' "$id")"
  nano="$(docker inspect --format '{{.HostConfig.NanoCpus}}' "$id")"
  pids="$(docker inspect --format '{{.HostConfig.PidsLimit}}' "$id")"
  {
    echo "service=$svc"
    echo "user=$uid"
    echo "readonly=$readonly_root"
    echo "security_opt=$(docker inspect --format '{{json .HostConfig.SecurityOpt}}' "$id")"
    echo "cap_drop=$caps"
    echo "memory=$mem"
    echo "nano_cpus=$nano"
    echo "pids_limit=$pids"
  } >"$RAW_DIR/isolation-$svc.txt"

  [[ "$uid" == "10001:10001" || "$uid" == "10001" ]] && pass "$svc user=$uid" || fail "$svc user=$uid (want 10001)"
  [[ "$readonly_root" == "true" ]] && pass "$svc read_only" || fail "$svc read_only=$readonly_root"
  echo "$caps" | grep -qi 'ALL' && pass "$svc cap_drop ALL" || fail "$svc cap_drop=$caps"
  docker inspect --format '{{json .HostConfig.SecurityOpt}}' "$id" | grep -q 'no-new-privileges' \
    && pass "$svc no-new-privileges" \
    || fail "$svc missing no-new-privileges"

  if [[ "$mem" != "0" && "$mem" != "<nil>" && -n "$mem" ]]; then
    pass "$svc memory limit=$mem"
  else
    fail "$svc memory limit missing/zero (HostConfig.Memory=$mem) — nested no-limit cannot Done"
  fi
  if [[ "$nano" != "0" && "$nano" != "<nil>" && -n "$nano" ]]; then
    pass "$svc cpu limit nanoCpus=$nano"
  else
    fail "$svc cpu limit missing/zero (HostConfig.NanoCpus=$nano)"
  fi
  if [[ "$pids" != "0" && "$pids" != "<nil>" && -n "$pids" ]]; then
    pass "$svc pids limit=$pids"
  else
    fail "$svc pids limit missing/zero (HostConfig.PidsLimit=$pids)"
  fi
done

# Known nested/nonstandard storage cannot qualify Done when used for boot-only.
case "$STORAGE_DRIVER" in
  vfs|fuse-overlayfs)
    fail "storage driver $STORAGE_DRIVER is nested/nonstandard — F02 Done requires standard host (e.g. overlay2)"
    ;;
  *)
    pass "storage driver $STORAGE_DRIVER"
    ;;
esac

# --- Convert network: Internal=true + executable egress probe ---
convert_id="$(service_id worker-convert)"
EGRESS_EXECUTED=0
EGRESS_BLOCKED=0
EGRESS_TOOL_MISSING=0
EGRESS_EXIT=""
EGRESS_RAW=""
CONVERT_NET=""
if [[ -n "${convert_id:-}" ]]; then
  nets="$(docker inspect --format '{{json .NetworkSettings.Networks}}' "$convert_id")"
  echo "$nets" >"$RAW_DIR/worker-convert-networks.json"
  CONVERT_NET="$(python3 -c 'import json,sys; nets=json.loads(sys.argv[1]);
print(next((k for k in nets if k.endswith("_convert") or k=="convert"), ""))' "$nets")"
  [[ -n "$CONVERT_NET" ]] && pass "worker-convert on convert network ($CONVERT_NET)" || fail "worker-convert missing convert network"
  if python3 -c 'import json,sys; nets=json.loads(sys.argv[1]);
raise SystemExit(0 if any(k == "edge" or k == "private" or k.endswith("_edge") or k.endswith("_private") for k in nets) else 1)' "$nets"; then
    fail "worker-convert attached to edge/private"
  else
    pass "worker-convert not on edge/private"
  fi

  # Soft curl-in-worker check is informational only — not a pass for egress.
  if docker exec "$convert_id" /bin/sh -c 'command -v curl >/dev/null 2>&1'; then
    note "worker-convert image has curl (not used as egress oracle)"
  else
    note "worker-convert image lacks curl — using external probe image on convert network"
  fi

  # Sandbox preflight inside convert container
  if docker exec "$convert_id" /usr/local/bin/fileconv-worker --sandbox-preflight \
    | tee "$RAW_DIR/sandbox-preflight.txt"; then
    pass "convert --sandbox-preflight"
  else
    fail "convert --sandbox-preflight"
  fi
fi

# Compose network inspect: convert is internal
docker network ls >"$RAW_DIR/networks.txt"
convert_net_id="$(docker network ls --format '{{.ID}} {{.Name}}' | awk '/convert$/ {print $1; exit}')"
if [[ -z "$convert_net_id" ]]; then
  convert_net_id="$(docker network ls --format '{{.ID}} {{.Name}}' | awk '/convert/ {print $1; exit}')"
fi
if [[ -n "$convert_net_id" ]]; then
  # Sanitize network inspect (drop potential Attachable noise; keep Internal + Name/Id).
  docker network inspect "$convert_net_id" | python3 -c '
import json,sys
from pathlib import Path
data=json.load(sys.stdin)
out=[]
for n in data:
    out.append({
        "Id": n.get("Id"),
        "Name": n.get("Name"),
        "Driver": n.get("Driver"),
        "Internal": n.get("Internal"),
        "Options": n.get("Options"),
        "Containers": {cid: {"Name": meta.get("Name")} for cid, meta in (n.get("Containers") or {}).items()},
    })
Path(sys.argv[1]).write_text(json.dumps(out, indent=2)+"\n", encoding="utf-8")
print("internal" if out and out[0].get("Internal") is True else "external")
print(out[0].get("Name","") if out else "")
' "$RAW_DIR/network-convert.json" >"$RAW_DIR/network-convert.meta"
  if head -n1 "$RAW_DIR/network-convert.meta" | grep -qx internal; then
    pass "convert network Internal=true"
  else
    fail "convert network not Internal"
  fi
  if [[ -z "$CONVERT_NET" ]]; then
    CONVERT_NET="$(sed -n '2p' "$RAW_DIR/network-convert.meta")"
  fi
else
  fail "convert network not found"
fi

# Executable egress probe on convert network (tool/image unavailable => FAIL, not soft-pass).
if [[ -n "$CONVERT_NET" ]]; then
  set +e
  PROBE_OUT="$(
    docker run --rm --network "$CONVERT_NET" "$EGRESS_PROBE_IMAGE" \
      wget -q -O /dev/null --timeout=3 --tries=1 https://1.1.1.1/ 2>&1
  )"
  EGRESS_EXIT=$?
  set -e
  EGRESS_RAW="exit=${EGRESS_EXIT}"$'\n'"${PROBE_OUT}"
  printf '%s\n' "$EGRESS_RAW" >"$RAW_DIR/egress-probe.txt"
  if [[ "$EGRESS_EXIT" -eq 125 || "$EGRESS_EXIT" -eq 127 ]]; then
    EGRESS_TOOL_MISSING=1
    fail "egress probe tool/image unavailable (exit=$EGRESS_EXIT) — not a soft-pass"
  elif [[ "$EGRESS_EXIT" -eq 0 ]]; then
    EGRESS_EXECUTED=1
    fail "convert network unexpected external egress (probe succeeded)"
  else
    EGRESS_EXECUTED=1
    EGRESS_BLOCKED=1
    pass "convert network external egress blocked (probe exit=$EGRESS_EXIT)"
  fi
else
  EGRESS_TOOL_MISSING=1
  fail "egress probe not executed — convert network name unknown"
  printf 'executed=false\nreason=convert_network_unknown\n' >"$RAW_DIR/egress-probe.txt"
fi

python3 - "$RAW_DIR/meta.json" "$EGRESS_EXECUTED" "$EGRESS_BLOCKED" "$EGRESS_TOOL_MISSING" "${EGRESS_EXIT:-}" "$CONVERT_NET" "$EGRESS_PROBE_IMAGE" "$RAW_DIR/egress-probe.txt" <<'PY'
import json, pathlib, sys
path, executed, blocked, missing, exit_code, network, image, raw_path = sys.argv[1:]
meta = json.loads(pathlib.Path(path).read_text(encoding="utf-8"))
raw = pathlib.Path(raw_path).read_text(encoding="utf-8", errors="replace")
meta["egressProbe"] = {
    "executed": executed == "1",
    "blocked": (blocked == "1") if executed == "1" else None,
    "toolMissing": missing == "1",
    "exitCode": int(exit_code) if exit_code not in ("", None) and str(exit_code).lstrip("-").isdigit() else None,
    "network": network,
    "probeImage": image,
    "raw": raw[:4000],
}
pathlib.Path(path).write_text(json.dumps(meta, indent=2) + "\n", encoding="utf-8")
PY

# --- API readiness body ---
curl -fsS "http://127.0.0.1:${MARKHAND_API_PORT:-8788}/api/v1/health/ready" \
  | tee "$RAW_DIR/api-ready.json" >/dev/null \
  && pass "api /health/ready" \
  || fail "api /health/ready"

# --- Separate images ---
api_image="$(docker inspect --format '{{.Config.Image}}' "$(service_id api)")"
worker_image="$(docker inspect --format '{{.Config.Image}}' "$(service_id worker-convert)")"
echo "api_image=$api_image" | tee "$RAW_DIR/images.txt"
echo "worker_image=$worker_image" | tee -a "$RAW_DIR/images.txt"
[[ "$api_image" != "$worker_image" ]] && pass "api/worker images distinct ($api_image vs $worker_image)" \
  || fail "api/worker share same image tag unexpectedly: $api_image"

# API image must not contain fileconv converter binary; worker must.
if docker exec "$(service_id api)" /bin/sh -c 'test ! -e /usr/local/bin/fileconv'; then
  pass "api image lacks fileconv converter"
else
  fail "api image unexpectedly contains /usr/local/bin/fileconv"
fi
if docker exec "$(service_id worker-convert)" /bin/sh -c 'test -x /usr/local/bin/fileconv && test -x /usr/local/bin/fileconv-worker'; then
  pass "worker image has fileconv + fileconv-worker"
else
  fail "worker image missing converter binaries"
fi
if docker exec "$(service_id worker-convert)" /bin/sh -c 'test ! -e /models/ggml-PhoWhisper-small.bin'; then
  pass "worker excludes PhoWhisper model path"
else
  fail "worker contains PhoWhisper model"
fi

# --- Native format smoke (txt/html/csv + optional pdf) via converter in worker ---
SMOKE_DIR="$RAW_DIR/format-smoke"
mkdir -p "$SMOKE_DIR"
printf 'Xin chào Markhand F02.\n' >"$SMOKE_DIR/sample.txt"
printf '<html><body><h1>Markhand</h1><p>POC F02</p></body></html>\n' >"$SMOKE_DIR/sample.html"
printf 'col_a,col_b\n1,hai\n' >"$SMOKE_DIR/sample.csv"
python3 - "$SMOKE_DIR/sample.png" <<'PY'
import pathlib, struct, sys, zlib
def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xffffffff)
png = (
    b"\x89PNG\r\n\x1a\n"
    + chunk(b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 2, 0, 0, 0))
    + chunk(b"IDAT", zlib.compress(b"\x00\xff\xff\xff"))
    + chunk(b"IEND", b"")
)
pathlib.Path(sys.argv[1]).write_bytes(png)
PY

worker_id="$(service_id worker-convert)"
if ! docker exec -u 10001:10001 "$worker_id" mkdir -p /tmp/format-smoke; then
  fail "cannot create worker tmpfs smoke directory"
fi
copy_smoke_file() {
  local source="$1"
  local destination="$2"
  if docker exec -i -u 10001:10001 "$worker_id" /bin/sh -c 'cat > "$1"' _ "$destination" <"$source"; then
    return 0
  fi
  fail "cannot stream $(basename "$source") into worker tmpfs"
  return 1
}
for fmt in txt html csv png; do
  out="$RAW_DIR/format-$fmt.md"
  if ! copy_smoke_file "$SMOKE_DIR/sample.$fmt" "/tmp/format-smoke/sample.$fmt"; then
    continue
  elif docker exec -u 10001:10001 "$worker_id" \
    /usr/local/bin/fileconv one "/tmp/format-smoke/sample.$fmt" \
    >"$out" 2>"$RAW_DIR/format-$fmt.err"; then
    if [[ -s "$out" ]]; then
      pass "native format smoke $fmt"
    else
      if [[ "$fmt" == "png" ]]; then
        pass "native format smoke png (empty OCR tolerated)"
      else
        fail "native format smoke $fmt produced empty output"
      fi
    fi
  else
    fail "native format smoke $fmt (see $RAW_DIR/format-$fmt.err)"
  fi
done

GOLD_PDF="$ROOT/bench/markhand_web/golden/documents/gold-004.pdf"
if [[ -f "$GOLD_PDF" ]]; then
  if ! copy_smoke_file "$GOLD_PDF" "/tmp/format-smoke/gold-004.pdf"; then
    fail "pdf smoke copy"
  elif docker exec -u 10001:10001 "$worker_id" \
    /usr/local/bin/fileconv one /tmp/format-smoke/gold-004.pdf \
    >"$RAW_DIR/format-pdf.md" 2>"$RAW_DIR/format-pdf.err"; then
    [[ -s "$RAW_DIR/format-pdf.md" ]] && pass "native format smoke pdf (gold-004)" \
      || fail "pdf smoke empty"
  else
    fail "native format smoke pdf"
  fi
else
  note "gold-004.pdf absent — skipped pdf smoke"
fi

# Finalize machine-readable report (sanitizes inspect again + redaction scan).
FINAL_ARGS=(
  python3 "$ROOT/deploy/scripts/poc_f02_boot_evidence.py" --finalize
  --json "$JSON"
  --md "$REPORT"
  --raw-dir "$RAW_DIR"
  --stamp "$STAMP"
  --fail "$FAIL"
  --compose-project "${MARKHAND_COMPOSE_PROJECT:-markhand-poc}"
)
if [[ "$NOLIMIT" -eq 1 ]]; then
  FINAL_ARGS+=(--nolimit-compose)
fi
set +e
"${FINAL_ARGS[@]}"
FINAL_RC=$?
set -e

if [[ "$FAIL" -ne 0 || "$FINAL_RC" -ne 0 ]]; then
  echo "POC boot evidence FAILED (shell_fail=$FAIL finalize_rc=$FINAL_RC) → $REPORT" >&2
  exit 1
fi
echo "POC boot evidence PASSED → $REPORT"
