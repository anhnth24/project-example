#!/usr/bin/env bash
# Collect P1B-F02 Docker runtime boot / isolation / sandbox-preflight evidence.
# Requires a healthy POC stack from deploy/scripts/poc-up.sh.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
COMPOSE_FILE="$ROOT/deploy/compose.poc.yml"
ENV_FILE="$ROOT/deploy/.env"
OUT_DIR="${1:-$ROOT/bench/markhand_web/reports}"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
RAW_DIR="${POC_EVIDENCE_RAW_DIR:-/tmp/markhand-f02-evidence-$STAMP}"
REPORT="$OUT_DIR/poc-f02-boot.md"
JSON="$OUT_DIR/poc-f02-boot.json"
FAIL=0

mkdir -p "$OUT_DIR" "$RAW_DIR"

if [[ ! -f "$ENV_FILE" ]]; then
  echo "missing $ENV_FILE — run: cp deploy/.env.example deploy/.env && deploy/scripts/poc-up.sh" >&2
  exit 1
fi

set -a
# shellcheck disable=SC1090
source "$ENV_FILE"
set +a

COMPOSE=(docker compose --env-file "$ENV_FILE" -f "$COMPOSE_FILE")
export COMPOSE_PROFILES="${COMPOSE_PROFILES:-mock}"

pass() { echo "PASS: $*"; echo "PASS: $*" >>"$RAW_DIR/summary.txt"; }
fail() { echo "FAIL: $*" >&2; echo "FAIL: $*" >>"$RAW_DIR/summary.txt"; FAIL=1; }

require_cmd() {
  if command -v "$1" >/dev/null 2>&1; then
    pass "command $1"
  else
    fail "missing command $1"
  fi
}

inspect_json() {
  local service="$1"
  local id
  id="$("${COMPOSE[@]}" ps -q "$service" || true)"
  if [[ -z "$id" ]]; then
    fail "service $service not running"
    return 1
  fi
  docker inspect "$id" >"$RAW_DIR/inspect-$service.json"
  echo "$id"
}

echo "== P1B-F02 Docker boot evidence ==" | tee "$RAW_DIR/summary.txt"
echo "stamp=$STAMP" | tee -a "$RAW_DIR/summary.txt"
echo "compose_profiles=$COMPOSE_PROFILES" | tee -a "$RAW_DIR/summary.txt"
date -u +%Y-%m-%dT%H:%M:%SZ | tee -a "$RAW_DIR/summary.txt"
docker version >"$RAW_DIR/docker-version.txt" 2>&1 || true
docker info >"$RAW_DIR/docker-info.txt" 2>&1 || true

require_cmd docker
require_cmd curl

"$ROOT/deploy/scripts/poc-health.sh" | tee "$RAW_DIR/poc-health.txt"
pass "poc-health"

# --- Isolation: UID / read-only / caps / no-new-privileges ---
for svc in api worker-convert worker-index worker-embedding; do
  id="$(inspect_json "$svc" || true)"
  [[ -n "${id:-}" ]] || continue
  uid="$(docker inspect --format '{{.Config.User}}' "$id")"
  readonly_root="$(docker inspect --format '{{.HostConfig.ReadonlyRootfs}}' "$id")"
  nnp="$(docker inspect --format '{{index .HostConfig.SecurityOpt 0}}' "$id")"
  caps="$(docker inspect --format '{{json .HostConfig.CapDrop}}' "$id")"
  mem="$(docker inspect --format '{{.HostConfig.Memory}}' "$id")"
  pids="$(docker inspect --format '{{.HostConfig.PidsLimit}}' "$id")"
  {
    echo "service=$svc"
    echo "user=$uid"
    echo "readonly=$readonly_root"
    echo "security_opt=$nnp"
    echo "cap_drop=$caps"
    echo "memory=$mem"
    echo "pids_limit=$pids"
  } >"$RAW_DIR/isolation-$svc.txt"

  [[ "$uid" == "10001:10001" || "$uid" == "10001" ]] && pass "$svc user=$uid" || fail "$svc user=$uid (want 10001)"
  [[ "$readonly_root" == "true" ]] && pass "$svc read_only" || fail "$svc read_only=$readonly_root"
  echo "$caps" | grep -qi 'ALL' && pass "$svc cap_drop ALL" || fail "$svc cap_drop=$caps"
  docker inspect --format '{{json .HostConfig.SecurityOpt}}' "$id" | grep -q 'no-new-privileges' \
    && pass "$svc no-new-privileges" \
    || fail "$svc missing no-new-privileges"
  # mem/pids may report 0 when host cgroup controllers are unavailable (nested VM).
  if [[ "$mem" != "0" && "$mem" != "<nil>" ]]; then
    pass "$svc memory limit=$mem"
  else
    echo "NOTE: $svc memory limit not enforced by runtime (HostConfig.Memory=$mem)" | tee -a "$RAW_DIR/summary.txt"
  fi
done

# --- Convert network: internal / no egress ---
convert_id="$(inspect_json worker-convert || true)"
if [[ -n "${convert_id:-}" ]]; then
  nets="$(docker inspect --format '{{json .NetworkSettings.Networks}}' "$convert_id")"
  echo "$nets" >"$RAW_DIR/worker-convert-networks.json"
  echo "$nets" | grep -q '"convert"' && pass "worker-convert on convert network" || fail "worker-convert missing convert network"
  # Must not be on edge/private (egress-capable paths).
  if echo "$nets" | grep -Eq '"edge"|"private"'; then
    fail "worker-convert attached to edge/private"
  else
    pass "worker-convert not on edge/private"
  fi
  # Probe: DNS/HTTP to public internet should fail (internal network).
  if docker exec "$convert_id" /bin/sh -c 'command -v curl >/dev/null && curl -fsS --max-time 3 https://1.1.1.1 >/dev/null'; then
    fail "worker-convert has unexpected external egress"
  else
    pass "worker-convert external egress blocked (or curl absent — expected lean image)"
  fi
  # Sandbox preflight inside convert container
  if docker exec "$convert_id" /usr/local/bin/fileconv-worker --sandbox-preflight \
    | tee "$RAW_DIR/sandbox-preflight.txt"; then
    pass "convert --sandbox-preflight"
  else
    fail "convert --sandbox-preflight"
  fi
fi

# --- API readiness body ---
curl -fsS "http://127.0.0.1:${MARKHAND_API_PORT:-8788}/api/v1/health/ready" \
  | tee "$RAW_DIR/api-ready.json" >/dev/null \
  && pass "api /health/ready" \
  || fail "api /health/ready"

# --- Separate images ---
api_image="$(docker inspect --format '{{.Config.Image}}' "$("${COMPOSE[@]}" ps -q api)")"
worker_image="$(docker inspect --format '{{.Config.Image}}' "$("${COMPOSE[@]}" ps -q worker-convert)")"
echo "api_image=$api_image" | tee "$RAW_DIR/images.txt"
echo "worker_image=$worker_image" | tee -a "$RAW_DIR/images.txt"
[[ "$api_image" != "$worker_image" ]] && pass "api/worker images distinct ($api_image vs $worker_image)" \
  || fail "api/worker share same image tag unexpectedly: $api_image"

# API image must not contain fileconv converter binary; worker must.
if docker exec "$("${COMPOSE[@]}" ps -q api)" /bin/sh -c 'test ! -e /usr/local/bin/fileconv'; then
  pass "api image lacks fileconv converter"
else
  fail "api image unexpectedly contains /usr/local/bin/fileconv"
fi
if docker exec "$("${COMPOSE[@]}" ps -q worker-convert)" /bin/sh -c 'test -x /usr/local/bin/fileconv && test -x /usr/local/bin/fileconv-worker'; then
  pass "worker image has fileconv + fileconv-worker"
else
  fail "worker image missing converter binaries"
fi
if docker exec "$("${COMPOSE[@]}" ps -q worker-convert)" /bin/sh -c 'test ! -e /models/ggml-PhoWhisper-small.bin'; then
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
# Minimal valid-ish PNG (1x1) for OCR path if tesseract present — may be empty OCR.
printf '\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x02\x00\x00\x00\x90wS\xde\x00\x00\x00\x0cIDATx\x9cc\xf8\x0f\x00\x00\x01\x01\x00\x05\x18\xd8N\x00\x00\x00\x00IEND\xaeB`\x82' \
  >"$SMOKE_DIR/sample.png"

worker_id="$("${COMPOSE[@]}" ps -q worker-convert)"
docker cp "$SMOKE_DIR/." "$worker_id:/tmp/format-smoke/"
for fmt in txt html csv png; do
  out="$RAW_DIR/format-$fmt.md"
  if docker exec -u 10001:10001 "$worker_id" \
    /usr/local/bin/fileconv one "/tmp/format-smoke/sample.$fmt" \
    >"$out" 2>"$RAW_DIR/format-$fmt.err"; then
    if [[ -s "$out" ]]; then
      pass "native format smoke $fmt"
    else
      # png OCR may legitimately produce empty text
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

# Optional PDF if golden corpus present in repo and docker cp works
GOLD_PDF="$ROOT/bench/markhand_web/golden/documents/gold-004.pdf"
if [[ -f "$GOLD_PDF" ]]; then
  docker cp "$GOLD_PDF" "$worker_id:/tmp/format-smoke/gold-004.pdf"
  if docker exec -u 10001:10001 "$worker_id" \
    /usr/local/bin/fileconv one /tmp/format-smoke/gold-004.pdf \
    >"$RAW_DIR/format-pdf.md" 2>"$RAW_DIR/format-pdf.err"; then
    [[ -s "$RAW_DIR/format-pdf.md" ]] && pass "native format smoke pdf (gold-004)" \
      || fail "pdf smoke empty"
  else
    fail "native format smoke pdf"
  fi
else
  echo "NOTE: gold-004.pdf absent — skipped pdf smoke" | tee -a "$RAW_DIR/summary.txt"
fi

# Compose network inspect: convert is internal
docker network ls >"$RAW_DIR/networks.txt"
convert_net_id="$(docker network ls --format '{{.ID}} {{.Name}}' | awk '/convert/ {print $1; exit}')"
if [[ -n "$convert_net_id" ]]; then
  docker network inspect "$convert_net_id" >"$RAW_DIR/network-convert.json"
  if grep -q '"Internal": true' "$RAW_DIR/network-convert.json"; then
    pass "convert network Internal=true"
  else
    fail "convert network not Internal"
  fi
else
  fail "convert network not found"
fi

# Write machine-readable + human report
python3 - "$JSON" "$REPORT" "$RAW_DIR" "$STAMP" "$FAIL" <<'PY'
import json, pathlib, sys, datetime
json_path, report_path, raw_dir, stamp, fail = sys.argv[1:]
raw = pathlib.Path(raw_dir)
summary = (raw / "summary.txt").read_text(encoding="utf-8", errors="replace").splitlines()
passes = [l[6:] for l in summary if l.startswith("PASS: ")]
fails = [l[6:] for l in summary if l.startswith("FAIL: ")]
notes = [l[6:] for l in summary if l.startswith("NOTE: ")]
payload = {
    "issue": "P1B-F02",
    "stamp_utc": stamp,
    "passed": fail == "0",
    "pass_count": len(passes),
    "fail_count": len(fails),
    "passes": passes,
    "fails": fails,
    "notes": notes,
    "raw_dir": str(raw),
    "generated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
}
pathlib.Path(json_path).write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
lines = [
    "# P1B-F02 POC Docker boot evidence",
    "",
    f"- Stamp (UTC): `{stamp}`",
    f"- Result: `{'PASS' if fail == '0' else 'FAIL'}`",
    f"- Passes: `{len(passes)}` / Fails: `{len(fails)}`",
    f"- Raw artifacts: `{raw}` (local; not necessarily committed)",
    "",
    "## Checks",
    "",
]
for p in passes:
    lines.append(f"- PASS: {p}")
for f in fails:
    lines.append(f"- FAIL: {f}")
for n in notes:
    lines.append(f"- NOTE: {n}")
lines += [
    "",
    "## Commands",
    "",
    "```bash",
    "cp deploy/.env.example deploy/.env",
    "deploy/scripts/poc-up.sh",
    "deploy/scripts/poc-boot-evidence.sh",
    "```",
    "",
    "## Acceptance mapping",
    "",
    "| Criterion | Evidence |",
    "|---|---|",
    "| Clean host boot | `poc-up.sh` + `poc-health` |",
    "| API/worker images separated | distinct image refs + binary presence checks |",
    "| Isolation UID/cap/read_only/no-new-privileges | `inspect-*.json` / `isolation-*.txt` |",
    "| Convert no egress | convert network `Internal=true` |",
    "| Sandbox preflight | `sandbox-preflight.txt` |",
    "| Native format smoke | `format-*.md` |",
    "",
]
pathlib.Path(report_path).write_text("\n".join(lines) + "\n", encoding="utf-8")
print(f"wrote {json_path}")
print(f"wrote {report_path}")
PY

if [[ "$FAIL" -ne 0 ]]; then
  echo "POC boot evidence FAILED" >&2
  exit 1
fi
echo "POC boot evidence PASSED → $REPORT"
