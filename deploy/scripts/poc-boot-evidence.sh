#!/usr/bin/env bash
# Collect P1B-F02 Docker runtime boot / isolation / sandbox-preflight evidence.
# Requires a healthy POC stack from deploy/scripts/poc-up.sh.
#
# Hermetic validator (no Docker / no secrets from deploy/.env):
#   deploy/scripts/poc-boot-evidence.sh --self-test
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
POC_PYTHON_BIN="${POC_PYTHON_BIN:-python3}"
if ! "$POC_PYTHON_BIN" -c 'pass' >/dev/null 2>&1 && command -v python >/dev/null 2>&1; then
  POC_PYTHON_BIN=python
fi

if [[ "${1:-}" == "--self-test" ]]; then
  exec "$POC_PYTHON_BIN" "$ROOT/deploy/scripts/poc_f02_boot_evidence.py" --self-test
fi

STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
RUN_ID="${POC_F02_RUN_ID:-f02-$STAMP-$$-$RANDOM}"
SOURCE_GIT_SHA_FULL="$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo unknown)"
SOURCE_GIT_SHA="$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)"
SOURCE_GIT_PORCELAIN="$(git -C "$ROOT" status --porcelain 2>/dev/null || true)"
export POC_F02_SOURCE_GIT_SHA_FULL="$SOURCE_GIT_SHA_FULL"
export POC_F02_SOURCE_GIT_SHA="$SOURCE_GIT_SHA"
if [[ -n "$SOURCE_GIT_PORCELAIN" ]]; then
  export POC_F02_SOURCE_GIT_DIRTY=1
else
  export POC_F02_SOURCE_GIT_DIRTY=0
fi
export POC_F02_SOURCE_GIT_PORCELAIN="$SOURCE_GIT_PORCELAIN"
if [[ -z "${MARKHAND_COMPOSE_PROJECT:-}" && "${POC_F02_CLEAN_BOOT:-}" == "1" ]]; then
  # Compose normalizes project labels to lowercase; record the effective value
  # up front so provenance validation compares like-for-like.
  export MARKHAND_COMPOSE_PROJECT="markhand-poc-${RUN_ID,,}"
fi

# shellcheck source=poc-compose.sh
source "$ROOT/deploy/scripts/poc-compose.sh"
poc_compose_init

OUT_DIR="${1:-${POC_F02_OUT_DIR:-$ROOT/.artifacts/markhand_web/reports}}"
RAW_PARENT="$OUT_DIR/phase-1b-gate/raw"
RAW_DIR="${POC_EVIDENCE_RAW_DIR:-$RAW_PARENT/$RUN_ID}"
REPORT="$OUT_DIR/poc-f02-boot.md"
JSON="$OUT_DIR/poc-f02-boot.json"
FAIL=0
NOLIMIT=0
if [[ -n "${POC_COMPOSE_EFFECTIVE:-}" ]]; then
  NOLIMIT=1
fi

# Pinned alpine already used by POC mock-embedding (images.lock.json).
EGRESS_PROBE_IMAGE="${POC_EGRESS_PROBE_IMAGE:-python:3.12.12-alpine@sha256:2d91681153dd4b8cdb52d4fd34a17b9edbafa4dd3086143cfd4b6c3a84c1acb0}"

mkdir -p "$OUT_DIR" "$RAW_PARENT"
if ! mkdir "$RAW_DIR"; then
  echo "exclusive F02 raw evidence directory already exists: $RAW_DIR" >&2
  exit 1
fi
LOCK_DIR="$RAW_DIR.lock"
if ! mkdir "$LOCK_DIR"; then
  echo "exclusive F02 lock already exists: $LOCK_DIR" >&2
  exit 1
fi
cleanup_lock() {
  rmdir "$LOCK_DIR" 2>/dev/null || true
}
_poc_add_exit_trap cleanup_lock

"$POC_PYTHON_BIN" "$ROOT/deploy/scripts/poc_f02_boot_evidence.py" --init-report \
  --json "$JSON" \
  --md "$REPORT" \
  --raw-dir "$RAW_DIR" \
  --stamp "$STAMP" \
  --compose-project "${MARKHAND_COMPOSE_PROJECT:-markhand-poc}" >/dev/null

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
  docker inspect "$id" | "$POC_PYTHON_BIN" -c '
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
  local id image_id repo_json mem nano pids cpu_quota cpu_period service_label project_label
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
  service_label="$(docker inspect --format '{{index .Config.Labels "com.docker.compose.service"}}' "$id")"
  project_label="$(docker inspect --format '{{index .Config.Labels "com.docker.compose.project"}}' "$id")"
  "$POC_PYTHON_BIN" - "$RAW_DIR/meta.json" "$service" "$id" "$image_id" "$repo_json" "$mem" "$nano" "$pids" "$cpu_quota" "$cpu_period" "$service_label" "$project_label" <<'PY'
import json, pathlib, sys
path, service, cid, image_id, repo_json, mem, nano, pids, quota, period, service_label, project_label = sys.argv[1:]
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
meta.setdefault("composeLabels", {})[service] = {
    "service": service_label,
    "project": project_label,
}
p.write_text(json.dumps(meta, indent=2) + "\n", encoding="utf-8")
PY
  "$POC_PYTHON_BIN" - "$RAW_DIR/meta.json" "$RAW_DIR/inspect-$service.json" "$service" <<'PY'
import json, pathlib, subprocess, sys

meta_path, inspect_path, service = sys.argv[1:]
meta = json.loads(pathlib.Path(meta_path).read_text(encoding="utf-8"))
inspect_items = json.loads(pathlib.Path(inspect_path).read_text(encoding="utf-8"))
item = inspect_items[0] if inspect_items else {}
host = item.get("HostConfig") or {}
networks = sorted(((item.get("NetworkSettings") or {}).get("Networks") or {}).keys())
internal = {}
for net in networks:
    try:
        value = subprocess.check_output(
            ["docker", "network", "inspect", "--format", "{{.Internal}}", net],
            text=True,
            stderr=subprocess.DEVNULL,
        ).strip()
        internal[net] = value == "true"
    except Exception:
        internal[net] = None
mounts = item.get("Mounts") or []
binds = [
    {"destination": m.get("Destination"), "mode": m.get("Mode"), "rw": m.get("RW")}
    for m in mounts
    if isinstance(m, dict) and m.get("Type") == "bind"
]
meta.setdefault("runtimeSecurity", {})[service] = {
    "user": ((item.get("Config") or {}).get("User")),
    "readOnlyRootfs": host.get("ReadonlyRootfs"),
    "securityOpt": host.get("SecurityOpt") or [],
    "capDrop": host.get("CapDrop") or [],
    "capAdd": host.get("CapAdd") or [],
    "privileged": host.get("Privileged"),
    "bindMounts": binds,
    "devices": host.get("Devices") or [],
    "tmpfs": host.get("Tmpfs") or {},
    "networks": networks,
    "networkInternal": internal,
}
pathlib.Path(meta_path).write_text(json.dumps(meta, indent=2) + "\n", encoding="utf-8")
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
COMPOSE_BLOB_SHA="$("$POC_PYTHON_BIN" - "$ROOT/deploy/compose.poc.yml" <<'PY'
import hashlib, pathlib, sys
print(hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest())
PY
)"

# Persist storage driver + seed meta.json (never log deploy/.env values).
STORAGE_DRIVER="$(docker info --format '{{.Driver}}' 2>/dev/null || echo unknown)"
"$POC_PYTHON_BIN" - "$RAW_DIR/meta.json" "$STORAGE_DRIVER" "${MARKHAND_COMPOSE_PROJECT:-markhand-poc}" "$NOLIMIT" "$COMPOSE_BLOB_SHA" <<'PY'
import json, pathlib, sys
path, driver, project, nolimit, compose_blob_sha = sys.argv[1:]
meta = {
    "storageDriver": driver,
    "composeProject": project,
    "composeProfiles": [part for part in __import__("re").split(r"[,\\s]+", __import__("os").environ.get("COMPOSE_PROFILES", "mock").strip()) if part] or ["mock"],
    "sourceGit": {
        "before": {
            "gitSha": __import__("os").environ.get("POC_F02_SOURCE_GIT_SHA", ""),
            "gitShaFull": __import__("os").environ.get("POC_F02_SOURCE_GIT_SHA_FULL", ""),
            "dirty": __import__("os").environ.get("POC_F02_SOURCE_GIT_DIRTY", "") == "1",
            "porcelain": __import__("os").environ.get("POC_F02_SOURCE_GIT_PORCELAIN", "").splitlines(),
        }
    },
    "nolimitComposeUsed": nolimit == "1",
    "composeBlobSha256": compose_blob_sha,
    "containerIds": {},
    "imageIds": {},
    "imageDigests": {},
    "composeLabels": {},
    "resourceLimits": {},
    "runtimeSecurity": {},
    "bootEvidence": {"cleanBootMeasured": False},
    "nativeSmoke": {"productionWorkerSandboxPath": False, "directFileconvContentAssertions": {}},
    "minioCredentialProbe": {},
    "qdrantInit": {},
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

if [[ "${POC_F02_CLEAN_BOOT:-}" == "1" ]]; then
  CLEAN_BOOT_TRANSCRIPT="$RAW_DIR/clean-boot.txt"
CLEAN_START="$("$POC_PYTHON_BIN" - <<'PY'
import time
print(time.monotonic())
PY
)"
  set +e
  {
    echo "clean_boot_project=${MARKHAND_COMPOSE_PROJECT:-markhand-poc}"
    echo "clean_boot_run_id=$RUN_ID"
    date -u +%Y-%m-%dT%H:%M:%SZ
    "${COMPOSE[@]}" down -v --remove-orphans
    # All three worker services share one image/build definition.
    "${COMPOSE[@]}" build api worker-convert
    "${COMPOSE[@]}" up -d
    "$ROOT/deploy/scripts/poc-health.sh"
    "${COMPOSE[@]}" ps
    date -u +%Y-%m-%dT%H:%M:%SZ
  } >"$CLEAN_BOOT_TRANSCRIPT" 2>&1
  CLEAN_RC=$?
  set -e
CLEAN_END="$("$POC_PYTHON_BIN" - <<'PY'
import time
print(time.monotonic())
PY
)"
CLEAN_DURATION="$("$POC_PYTHON_BIN" - "$CLEAN_START" "$CLEAN_END" <<'PY'
import sys
print(max(0.0, float(sys.argv[2]) - float(sys.argv[1])))
PY
)"
  "$POC_PYTHON_BIN" - "$RAW_DIR/meta.json" "$CLEAN_RC" "$CLEAN_DURATION" "clean-boot.txt" "${MARKHAND_COMPOSE_PROJECT:-markhand-poc}" "$RUN_ID" <<'PY'
import json, pathlib, sys
path, rc, duration, transcript, project, run_id = sys.argv[1:]
meta = json.loads(pathlib.Path(path).read_text(encoding="utf-8"))
meta["bootEvidence"] = {
    "cleanBootMeasured": rc == "0",
    "exitCode": int(rc),
    "durationSeconds": float(duration),
    "transcript": transcript,
    "freshVolumes": True,
    "readinessChecked": rc == "0",
    "uniqueComposeProject": run_id.lower() in project.lower(),
}
pathlib.Path(path).write_text(json.dumps(meta, indent=2) + "\n", encoding="utf-8")
PY
  if [[ "$CLEAN_RC" -eq 0 ]]; then
    pass "clean project boot measured (${CLEAN_DURATION}s)"
  else
    fail "clean project boot failed (see $CLEAN_BOOT_TRANSCRIPT)"
  fi
else
  fail "clean project boot not measured (set POC_F02_CLEAN_BOOT=1 to prove lifecycle timing)"
fi

"$ROOT/deploy/scripts/poc-health.sh" | tee "$RAW_DIR/poc-health.txt"
pass "poc-health"

# --- Metadata for expected O04 services + limit surfaces ---
for svc in api minio postgres qdrant mock-embedding worker-convert worker-index worker-embedding worker-delete worker-reconcile; do
  collect_service_meta "$svc" || true
done

for svc in postgres qdrant minio mock-embedding; do
  id="$(service_id "$svc")"
  if [[ -z "$id" ]]; then
    fail "$svc not running for long-lived resource validation"
    continue
  fi
  mem="$(docker inspect --format '{{.HostConfig.Memory}}' "$id")"
  nano="$(docker inspect --format '{{.HostConfig.NanoCpus}}' "$id")"
  pids="$(docker inspect --format '{{.HostConfig.PidsLimit}}' "$id")"
  if [[ "$mem" != "0" && "$mem" != "<nil>" && -n "$mem" ]]; then
    pass "$svc memory limit=$mem"
  else
    fail "$svc memory limit missing/zero (HostConfig.Memory=$mem)"
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
if [[ "${COMPOSE_PROFILES}" == *aiteamvn* ]]; then
  collect_service_meta "embedding-cpu" || true
fi

# --- Isolation: UID / read-only / caps / no-new-privileges / nonzero limits ---
for svc in api worker-convert worker-index worker-embedding worker-delete worker-reconcile; do
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

  if [[ "$uid" == "10001:10001" || "$uid" == "10001" ]]; then
    pass "$svc user=$uid"
  else
    fail "$svc user=$uid (want 10001)"
  fi
  if [[ "$readonly_root" == "true" ]]; then
    pass "$svc read_only"
  else
    fail "$svc read_only=$readonly_root"
  fi
  if echo "$caps" | grep -qi 'ALL'; then
    pass "$svc cap_drop ALL"
  else
    fail "$svc cap_drop=$caps"
  fi
  if docker inspect --format '{{json .HostConfig.SecurityOpt}}' "$id" | grep -q 'no-new-privileges'; then
    pass "$svc no-new-privileges"
  else
    fail "$svc missing no-new-privileges"
  fi

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
EGRESS_DEFAULT_ROUTE_PRESENT=""
CONVERT_NET=""
if [[ -n "${convert_id:-}" ]]; then
  nets="$(docker inspect --format '{{json .NetworkSettings.Networks}}' "$convert_id")"
  echo "$nets" >"$RAW_DIR/worker-convert-networks.json"
  CONVERT_NET="$("$POC_PYTHON_BIN" -c 'import json,sys; nets=json.loads(sys.argv[1]); keys=sorted(nets.keys());
valid=[k for k in keys if k == "convert" or k.endswith("_convert")]
print(valid[0] if len(keys) == 1 and len(valid) == 1 else "")' "$nets")"
  if [[ -n "$CONVERT_NET" ]]; then
    pass "worker-convert exactly on convert network ($CONVERT_NET)"
  else
    fail "worker-convert has unexpected network membership: $nets"
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
    "$POC_PYTHON_BIN" - "$RAW_DIR/meta.json" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1])
meta = json.loads(path.read_text(encoding="utf-8"))
meta.setdefault("nativeSmoke", {})["workerSandboxPreflight"] = True
path.write_text(json.dumps(meta, indent=2) + "\n", encoding="utf-8")
PY
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
  docker network inspect "$convert_net_id" | "$POC_PYTHON_BIN" -c '
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

# Executable route probe from the actual worker network namespace.
if [[ -n "$CONVERT_NET" && -n "${convert_id:-}" ]]; then
  set +e
  PROBE_OUT="$(
    docker run --rm --network "container:$convert_id" "$EGRESS_PROBE_IMAGE" \
      python -c 'import errno, socket, sys
default_route = False
try:
    with open("/proc/net/route", encoding="utf-8") as route:
        for line in route.read().splitlines()[1:]:
            parts = line.split()
            if len(parts) >= 2 and parts[1] == "00000000":
                default_route = True
                break
except OSError as e:
    print(f"ROUTE_READ_ERROR {type(e).__name__}: {e}")
    sys.exit(31)
print(f"DEFAULT_ROUTE_PRESENT={str(default_route).lower()}")
if default_route:
    sys.exit(11)
s = socket.socket()
s.settimeout(3)
try:
    s.connect(("1.1.1.1", 443))
    print("CONNECTED")
    sys.exit(10)
except OSError as e:
    code = getattr(e, "errno", None)
    print(f"OSERROR errno={code} name={errno.errorcode.get(code, type(e).__name__)} msg={e}")
    if code in (errno.ENETUNREACH, errno.EHOSTUNREACH) or isinstance(e, TimeoutError) or "timed out" in str(e).lower():
        sys.exit(20)
    if code == errno.ECONNREFUSED:
        sys.exit(10)
    sys.exit(30)' 2>&1
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
    fail "egress probe returned impossible success code"
  elif [[ "$EGRESS_EXIT" -eq 10 ]]; then
    EGRESS_EXECUTED=1
    fail "convert worker namespace unexpected external egress (route connected)"
  elif [[ "$EGRESS_EXIT" -eq 11 ]]; then
    EGRESS_EXECUTED=1
    EGRESS_DEFAULT_ROUTE_PRESENT=1
    fail "convert worker namespace has default route"
  elif [[ "$EGRESS_EXIT" -eq 20 ]]; then
    EGRESS_EXECUTED=1
    EGRESS_BLOCKED=1
    pass "convert worker namespace external route blocked (probe exit=$EGRESS_EXIT)"
  else
    EGRESS_EXECUTED=1
    fail "convert worker namespace egress probe inconclusive (exit=$EGRESS_EXIT)"
  fi
else
  EGRESS_TOOL_MISSING=1
  fail "egress probe not executed — convert worker namespace unknown"
  printf 'executed=false\nreason=convert_worker_namespace_unknown\n' >"$RAW_DIR/egress-probe.txt"
fi
if grep -q 'DEFAULT_ROUTE_PRESENT=true' "$RAW_DIR/egress-probe.txt" 2>/dev/null; then
  EGRESS_DEFAULT_ROUTE_PRESENT=1
elif grep -q 'DEFAULT_ROUTE_PRESENT=false' "$RAW_DIR/egress-probe.txt" 2>/dev/null; then
  EGRESS_DEFAULT_ROUTE_PRESENT=0
fi

"$POC_PYTHON_BIN" - "$RAW_DIR/meta.json" "$EGRESS_EXECUTED" "$EGRESS_BLOCKED" "$EGRESS_TOOL_MISSING" "${EGRESS_EXIT:-}" "$CONVERT_NET" "$EGRESS_PROBE_IMAGE" "$RAW_DIR/egress-probe.txt" "${EGRESS_DEFAULT_ROUTE_PRESENT:-}" <<'PY'
import json, pathlib, sys
path, executed, blocked, missing, exit_code, network, image, raw_path, default_route = sys.argv[1:]
meta = json.loads(pathlib.Path(path).read_text(encoding="utf-8"))
raw = pathlib.Path(raw_path).read_text(encoding="utf-8", errors="replace")
default_route_present = None
if default_route == "1":
    default_route_present = True
elif default_route == "0":
    default_route_present = False
meta["egressProbe"] = {
    "executed": executed == "1",
    "blocked": (blocked == "1") if executed == "1" else None,
    "toolMissing": missing == "1",
    "exitCode": int(exit_code) if exit_code not in ("", None) and str(exit_code).lstrip("-").isdigit() else None,
    "network": network,
    "namespace": "container:worker-convert",
    "probeImage": image,
    "routeProbe": {
        "target": "1.1.1.1:443",
        "blocked": blocked == "1",
        "classification": "route_blocked" if blocked == "1" else ("route_connected" if exit_code == "10" else "inconclusive"),
        "defaultRoutePresent": default_route_present,
    },
    "raw": raw[:4000],
}
pathlib.Path(path).write_text(json.dumps(meta, indent=2) + "\n", encoding="utf-8")
PY

# --- API readiness body ---
if curl -fsS "http://127.0.0.1:${MARKHAND_API_PORT:-8788}/api/v1/health/ready" \
  | tee "$RAW_DIR/api-ready.json" >/dev/null; then
  pass "api /health/ready"
else
  fail "api /health/ready"
fi

# --- MinIO narrow application credential runtime probe (no secrets in transcript) ---
PRIVATE_NET="$(docker inspect --format '{{json .NetworkSettings.Networks}}' "$(service_id api)" | "$POC_PYTHON_BIN" -c 'import json,sys; nets=json.loads(sys.stdin.read()); print(next((k for k in nets if k=="private" or k.endswith("_private")), ""))')"
MINIO_MC_IMAGE="${POC_MINIO_MC_IMAGE:-minio/mc:RELEASE.2025-08-13T08-35-41Z@sha256:a7fe349ef4bd8521fb8497f55c6042871b2ae640607cf99d9bede5e9bdf11727}"
MINIO_POSITIVE=0
MINIO_NEGATIVE=0
MINIO_CROSS_BUCKET_NEGATIVE=0
MINIO_ADMIN_DENIAL_KIND=""
MINIO_CROSS_DENIAL_KIND=""
if [[ -n "$PRIVATE_NET" ]]; then
  set +e
  docker run --rm --network "$PRIVATE_NET" \
    -e MARKHAND_MINIO_ACCESS_KEY \
    -e MARKHAND_MINIO_SECRET_KEY \
    -e MARKHAND_MINIO_BUCKET \
    --entrypoint /bin/sh \
    "$MINIO_MC_IMAGE" -c '
      set -eu
      export HOME=/tmp MC_CONFIG_DIR=/tmp/mc
      mc alias set app http://minio:9000 "$MARKHAND_MINIO_ACCESS_KEY" "$MARKHAND_MINIO_SECRET_KEY" >/dev/null
      mc ls "app/${MARKHAND_MINIO_BUCKET:-markhand-documents}" >/dev/null
    ' >"$RAW_DIR/minio-app-positive.txt" 2>&1
  MINIO_POSITIVE_RC=$?
  docker run --rm --network "$PRIVATE_NET" \
    -e MARKHAND_MINIO_ACCESS_KEY \
    -e MARKHAND_MINIO_SECRET_KEY \
    --entrypoint /bin/sh \
    "$MINIO_MC_IMAGE" -c '
      set -eu
      export HOME=/tmp MC_CONFIG_DIR=/tmp/mc
      mc alias set app http://minio:9000 "$MARKHAND_MINIO_ACCESS_KEY" "$MARKHAND_MINIO_SECRET_KEY" >/dev/null
      mc admin user list app
    ' >"$RAW_DIR/minio-app-negative.txt" 2>&1
  MINIO_NEGATIVE_RC=$?
  docker run --rm --network "$PRIVATE_NET" \
    -e MARKHAND_MINIO_ACCESS_KEY \
    -e MARKHAND_MINIO_SECRET_KEY \
    --entrypoint /bin/sh \
    "$MINIO_MC_IMAGE" -c '
      set -eu
      export HOME=/tmp MC_CONFIG_DIR=/tmp/mc
      mc alias set app http://minio:9000 "$MARKHAND_MINIO_ACCESS_KEY" "$MARKHAND_MINIO_SECRET_KEY" >/dev/null
      mc ls "app/markhand-f02-denied-bucket"
    ' >"$RAW_DIR/minio-app-cross-bucket-negative.txt" 2>&1
  MINIO_CROSS_BUCKET_RC=$?
  set -e
  [[ "$MINIO_POSITIVE_RC" -eq 0 ]] && MINIO_POSITIVE=1
  MINIO_ADMIN_DENIAL_KIND="$("$POC_PYTHON_BIN" - "$RAW_DIR/minio-app-negative.txt" "$MINIO_NEGATIVE_RC" <<'PY'
import pathlib, re, sys
text = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8", errors="replace")
rc = int(sys.argv[2])
if rc == 0:
    print("allowed")
elif re.search(r"(?i)(access.?denied|not authorized|forbidden|403|insufficient permissions)", text):
    print("authorization_denied")
else:
    print("other_error")
PY
)"
  MINIO_CROSS_DENIAL_KIND="$("$POC_PYTHON_BIN" - "$RAW_DIR/minio-app-cross-bucket-negative.txt" "$MINIO_CROSS_BUCKET_RC" <<'PY'
import pathlib, re, sys
text = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8", errors="replace")
rc = int(sys.argv[2])
if rc == 0:
    print("allowed")
elif re.search(r"(?i)(access.?denied|not authorized|forbidden|403|insufficient permissions)", text):
    print("authorization_denied")
else:
    print("other_error")
PY
)"
  [[ "$MINIO_ADMIN_DENIAL_KIND" == "authorization_denied" ]] && MINIO_NEGATIVE=1
  [[ "$MINIO_CROSS_DENIAL_KIND" == "authorization_denied" ]] && MINIO_CROSS_BUCKET_NEGATIVE=1
  if [[ "$MINIO_POSITIVE" -eq 1 ]]; then
    pass "MinIO app credential can list configured bucket"
  else
    fail "MinIO app credential positive bucket probe failed"
  fi
  if [[ "$MINIO_NEGATIVE" -eq 1 ]]; then
    pass "MinIO app credential admin user list authorization denied"
  else
    fail "MinIO app credential admin user list denial not proven (kind=$MINIO_ADMIN_DENIAL_KIND)"
  fi
  if [[ "$MINIO_CROSS_BUCKET_NEGATIVE" -eq 1 ]]; then
    pass "MinIO app credential cross-bucket access authorization denied"
  else
    fail "MinIO app credential cross-bucket denial not proven (kind=$MINIO_CROSS_DENIAL_KIND)"
  fi
else
  fail "MinIO credential probe skipped: private network unknown"
fi
"$POC_PYTHON_BIN" - "$RAW_DIR/meta.json" "$MINIO_POSITIVE" "$MINIO_NEGATIVE" "$MINIO_CROSS_BUCKET_NEGATIVE" "$MINIO_ADMIN_DENIAL_KIND" "$MINIO_CROSS_DENIAL_KIND" <<'PY'
import json, pathlib, sys
path, positive, negative, cross_negative, admin_kind, cross_kind = sys.argv[1:]
meta = json.loads(pathlib.Path(path).read_text(encoding="utf-8"))
meta["minioCredentialProbe"] = {
    "positiveListBucket": positive == "1",
    "negativeAdminDenied": negative == "1",
    "negativeCrossBucketDenied": cross_negative == "1",
    "adminDenialKind": admin_kind,
    "crossBucketDenialKind": cross_kind,
}
pathlib.Path(path).write_text(json.dumps(meta, indent=2) + "\n", encoding="utf-8")
PY

# --- Qdrant init exit/config evidence ---
QDRANT_INIT_ID="$("${COMPOSE[@]}" ps --all -q qdrant-init || true)"
QDRANT_INIT_EXIT=""
QDRANT_CONFIG_OK=0
if [[ -n "$QDRANT_INIT_ID" ]]; then
  QDRANT_INIT_EXIT="$(docker inspect --format '{{.State.ExitCode}}' "$QDRANT_INIT_ID")"
  docker logs "$QDRANT_INIT_ID" >"$RAW_DIR/qdrant-init.log" 2>&1 || true
else
  QDRANT_INIT_EXIT="-1"
fi
set +e
"$POC_PYTHON_BIN" - "$RAW_DIR/qdrant-config.json" "${MARKHAND_INDEX_SIGNATURE:-72dda20007ffb7fbe293612091103321eb9e4e0e4a0517a5f3413e31a2978874}" "${MARKHAND_EMBEDDING_DIMENSIONS:-8}" "http://127.0.0.1:${MARKHAND_QDRANT_HTTP_PORT:-6343}" <<'PY'
import json, pathlib, sys, urllib.request
out, sig, dims, base = sys.argv[1:]
name = f"markhand_chunks_{sig}"
result = {"collection": name, "expectedDimensions": int(dims), "configVerified": False}
try:
    with urllib.request.urlopen(f"{base}/collections/{name}", timeout=3) as response:
        payload = json.loads(response.read().decode(errors="replace"))
    vectors = payload.get("result", {}).get("config", {}).get("params", {}).get("vectors", {})
    result["actualDimensions"] = vectors.get("size")
    result["distance"] = vectors.get("distance")
    result["configVerified"] = vectors.get("size") == int(dims) and str(vectors.get("distance", "")).lower() == "cosine"
except Exception as error:
    result["error"] = type(error).__name__
pathlib.Path(out).write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
raise SystemExit(0 if result["configVerified"] else 1)
PY
QDRANT_CONFIG_RC=$?
set -e
[[ "$QDRANT_CONFIG_RC" -eq 0 ]] && QDRANT_CONFIG_OK=1
if [[ "$QDRANT_INIT_EXIT" == "0" ]]; then
  pass "qdrant-init exited 0"
else
  fail "qdrant-init exit=$QDRANT_INIT_EXIT"
fi
if [[ "$QDRANT_CONFIG_OK" -eq 1 ]]; then
  pass "Qdrant collection config verified"
else
  fail "Qdrant collection config not verified"
fi
"$POC_PYTHON_BIN" - "$RAW_DIR/meta.json" "$QDRANT_INIT_EXIT" "$QDRANT_CONFIG_OK" "${MARKHAND_INDEX_SIGNATURE:-72dda20007ffb7fbe293612091103321eb9e4e0e4a0517a5f3413e31a2978874}" <<'PY'
import json, pathlib, sys
path, exit_code, config_ok, index_signature = sys.argv[1:]
meta = json.loads(pathlib.Path(path).read_text(encoding="utf-8"))
meta["qdrantInit"] = {
    "exitCode": int(exit_code) if str(exit_code).lstrip("-").isdigit() else None,
    "configVerified": config_ok == "1",
    "configEvidence": "qdrant-config.json",
    "indexSignature": index_signature,
}
pathlib.Path(path).write_text(json.dumps(meta, indent=2) + "\n", encoding="utf-8")
PY

# --- Separate images ---
api_image="$(docker inspect --format '{{.Config.Image}}' "$(service_id api)")"
worker_image="$(docker inspect --format '{{.Config.Image}}' "$(service_id worker-convert)")"
echo "api_image=$api_image" | tee "$RAW_DIR/images.txt"
echo "worker_image=$worker_image" | tee -a "$RAW_DIR/images.txt"
if [[ "$api_image" != "$worker_image" ]]; then
  pass "api/worker images distinct ($api_image vs $worker_image)"
else
  fail "api/worker share same image tag unexpectedly: $api_image"
fi

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

# --- Native format smoke matrix through the worker's production sandbox runner.
SMOKE_DIR="$RAW_DIR/format-smoke"
mkdir -p "$SMOKE_DIR"
printf 'Xin chào Markhand F02.\n' >"$SMOKE_DIR/sample.txt"
printf '<html><body><h1>Markhand</h1><p>POC F02</p></body></html>\n' >"$SMOKE_DIR/sample.html"
printf 'col_a,col_b\n1,hai\n' >"$SMOKE_DIR/sample.csv"
"$POC_PYTHON_BIN" - "$SMOKE_DIR/sample.png" <<'PY'
import math, pathlib, struct, sys, zlib

width, height = 600, 220
pixels = bytearray([255]) * (width * height)

def black(x, y):
    if 0 <= x < width and 0 <= y < height:
        pixels[y * width + x] = 0

def rectangle(x0, y0, x1, y1):
    for y in range(y0, y1):
        for x in range(x0, x1):
            black(x, y)

def ring(cx, cy, outer_x, outer_y, inner_x, inner_y):
    for y in range(cy - outer_y, cy + outer_y + 1):
        for x in range(cx - outer_x, cx + outer_x + 1):
            outer = ((x - cx) / outer_x) ** 2 + ((y - cy) / outer_y) ** 2
            inner = ((x - cx) / inner_x) ** 2 + ((y - cy) / inner_y) ** 2
            if outer <= 1 and inner >= 1:
                black(x, y)

def stroke(points, radius):
    for start, end in zip(points, points[1:]):
        x0, y0 = start
        x1, y1 = end
        dx, dy = x1 - x0, y1 - y0
        length2 = dx * dx + dy * dy
        for y in range(min(y0, y1) - radius, max(y0, y1) + radius + 1):
            for x in range(min(x0, x1) - radius, max(x0, x1) + radius + 1):
                t = max(0.0, min(1.0, ((x - x0) * dx + (y - y0) * dy) / length2))
                if math.hypot(x - (x0 + t * dx), y - (y0 + t * dy)) <= radius:
                    black(x, y)

# Large, conventional sans-serif shapes that OCR engines read consistently.
rectangle(55, 30, 78, 190)
rectangle(55, 30, 170, 53)
rectangle(55, 95, 150, 118)
ring(260, 110, 65, 82, 39, 57)
stroke([(365, 62), (390, 38), (462, 38), (490, 62), (490, 88), (370, 180), (505, 180)], 12)

def chunk(kind, data):
    checksum = zlib.crc32(kind + data) & 0xFFFFFFFF
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", checksum)

raw = b"".join(b"\x00" + bytes(pixels[y * width:(y + 1) * width]) for y in range(height))
png = (
    b"\x89PNG\r\n\x1a\n"
    + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 0, 0, 0, 0))
    + chunk(b"IDAT", zlib.compress(raw, 9))
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
declare -A SMOKE_SOURCE=(
  [txt]="$SMOKE_DIR/sample.txt"
  [html]="$SMOKE_DIR/sample.html"
  [csv]="$SMOKE_DIR/sample.csv"
  [png]="$SMOKE_DIR/sample.png"
  [pdf]="$ROOT/bench/markhand_web/golden/documents/gold-004.pdf"
  [docx]="$ROOT/bench/markhand_web/golden/documents/gold-006.docx"
  [pptx]="$ROOT/bench/markhand_web/golden/documents/gold-009.pptx"
  [xlsx]="$ROOT/bench/markhand_web/golden/documents/gold-011.xlsx"
)
for fmt in csv docx html pdf png pptx txt xlsx; do
  out="$RAW_DIR/format-$fmt.md"
  source_path="${SMOKE_SOURCE[$fmt]}"
  if [[ ! -f "$source_path" ]]; then
    fail "native format smoke $fmt source missing"
    continue
  fi
  if ! copy_smoke_file "$source_path" "/tmp/format-smoke/sample.$fmt"; then
    continue
  elif docker exec -u 10001:10001 "$worker_id" \
    /usr/local/bin/fileconv-worker --sandbox-convert-probe "/tmp/format-smoke/sample.$fmt" \
    >"$out" 2>"$RAW_DIR/format-$fmt.err"; then
    case "$fmt" in
      docx)
        if grep -qi 'HS-2028-006' "$out"; then
          pass "native format smoke $fmt content"
        else
          fail "native format smoke $fmt content assertion"
        fi
        ;;
      pdf)
        if grep -qi 'HS-2026-004' "$out"; then
          pass "native format smoke $fmt content"
        else
          fail "native format smoke $fmt content assertion"
        fi
        ;;
      pptx)
        if grep -qi 'HS-2028-009' "$out"; then
          pass "native format smoke $fmt content"
        else
          fail "native format smoke $fmt content assertion"
        fi
        ;;
      txt)
        if grep -qi 'Markhand F02' "$out"; then
          pass "native format smoke $fmt content"
        else
          fail "native format smoke $fmt content assertion"
        fi
        ;;
      html)
        if grep -qi 'POC F02' "$out"; then
          pass "native format smoke $fmt content"
        else
          fail "native format smoke $fmt content assertion"
        fi
        ;;
      csv)
        if grep -qi 'col_a' "$out" && grep -qi 'hai' "$out"; then
          pass "native format smoke $fmt content"
        else
          fail "native format smoke $fmt content assertion"
        fi
        ;;
      png)
        if grep -qi 'FO2' "$out"; then
          pass "native format smoke $fmt OCR content"
        else
          fail "native format smoke $fmt OCR content assertion"
        fi
        ;;
      xlsx)
        if grep -qi 'HS-2027-011' "$out"; then
          pass "native format smoke $fmt content"
        else
          fail "native format smoke $fmt content assertion"
        fi
        ;;
    esac
  else
    fail "native format smoke $fmt (see $RAW_DIR/format-$fmt.err)"
  fi
done

"$POC_PYTHON_BIN" - "$RAW_DIR/meta.json" "$RAW_DIR" <<'PY'
import json, pathlib, sys
meta_path = pathlib.Path(sys.argv[1])
raw_dir = pathlib.Path(sys.argv[2])
meta = json.loads(meta_path.read_text(encoding="utf-8"))
assertions = {}
markers = {
    "csv": ("col_a", "hai"),
    "docx": ("HS-2028-006",),
    "html": ("POC F02",),
    "pdf": ("HS-2026-004",),
    "png": ("FO2",),
    "pptx": ("HS-2028-009",),
    "txt": ("Markhand F02",),
    "xlsx": ("HS-2027-011",),
}
for fmt, expected in markers.items():
    out = raw_dir / f"format-{fmt}.md"
    text = out.read_text(encoding="utf-8", errors="replace") if out.is_file() else ""
    assertions[fmt] = bool(text.strip()) and all(marker.lower() in text.lower() for marker in expected)
meta.setdefault("nativeSmoke", {})["directFileconvContentAssertions"] = assertions
meta.setdefault("nativeSmoke", {})["contentAssertions"] = assertions
meta.setdefault("nativeSmoke", {})["productionWorkerSandboxPath"] = all(assertions.values())
meta.setdefault("nativeSmoke", {})["workerSandboxConversionProbe"] = "/usr/local/bin/fileconv-worker --sandbox-convert-probe"
meta_path.write_text(json.dumps(meta, indent=2) + "\n", encoding="utf-8")
PY

# Finalize machine-readable report (sanitizes inspect again + redaction scan).
FINAL_ARGS=(
  "$POC_PYTHON_BIN" "$ROOT/deploy/scripts/poc_f02_boot_evidence.py" --finalize
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
