#!/usr/bin/env bash
# Seed two-org Phase 1C fixture through production HTTP APIs + controlled POC DB bootstrap.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

: "${MARKHAND_API_BASE:=http://127.0.0.1:${MARKHAND_API_PORT:-8788}}"
: "${MARKHAND_PHASE1C_SEED_JSON:=$ROOT/.artifacts/phase1c-multi-org-seed.json}"
: "${MARKHAND_PHASE1C_CREDENTIALS_JSON:=$ROOT/.artifacts/phase1c-multi-org-seed.credentials.json}"

export MARKHAND_API_BASE MARKHAND_PHASE1C_SEED_JSON MARKHAND_PHASE1C_CREDENTIALS_JSON ROOT

purge_phase1c_credentials() {
  python3 - <<'PY'
import os
import sys
from pathlib import Path

root = Path(os.environ["ROOT"])
sys.path.insert(0, str(root / "bench/markhand_web/scripts"))
from phase1c_deployed_probes import purge_phase1c_credentials

cred_path = Path(os.environ.get("MARKHAND_PHASE1C_CREDENTIALS_JSON", ""))
if cred_path:
    purge_phase1c_credentials(cred_path)
PY
}

trap purge_phase1c_credentials EXIT INT TERM

mkdir -p "$(dirname "$MARKHAND_PHASE1C_SEED_JSON")"

python3 bench/markhand_web/scripts/phase1c_multi_org_seed.py

if [[ ! -f "$MARKHAND_PHASE1C_SEED_JSON" ]]; then
  echo "seed evidence missing" >&2
  exit 1
fi
if [[ ! -f "$MARKHAND_PHASE1C_CREDENTIALS_JSON" ]]; then
  echo "seed credentials missing" >&2
  exit 1
fi

python3 - <<'PY'
import json
import os
import sys
from pathlib import Path

root = Path(os.environ["ROOT"])
sys.path.insert(0, str(root / "bench/markhand_web/scripts"))
from phase1c_deployed_probes import build_public_seed_evidence

seed_path = Path(os.environ["MARKHAND_PHASE1C_SEED_JSON"])
payload = json.loads(seed_path.read_text(encoding="utf-8"))
build_public_seed_evidence(payload)
cred_path = Path(os.environ["MARKHAND_PHASE1C_CREDENTIALS_JSON"])
if oct(cred_path.stat().st_mode & 0o777) not in {"0o600", "0o400"}:
    cred_path.chmod(0o600)
PY

trap - EXIT INT TERM

echo "PHASE1C_SEED_EOF"
