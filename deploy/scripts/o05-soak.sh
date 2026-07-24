#!/usr/bin/env bash
# P1B-O05 measured mixed-load soak runner.
# Writes o05-soak.json/.md + raw/o05-<stamp>/ — never O04 artifacts.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

if [[ "${1:-}" == "--self-test" ]]; then
  exec python3 bench/markhand_web/soak/run_soak.py --self-test
fi

echo "O05 soak (MARKHAND_SOAK=${MARKHAND_SOAK:-unset})"
args=(
  --profile bench/markhand_web/workloads/phase1b-mixed.yaml
  --out "${MARKHAND_O05_OUT_DIR:-bench/markhand_web/reports/phase-1b-gate}"
  --f02 "${MARKHAND_O05_F02_REPORT:-bench/markhand_web/reports/poc-f02-boot.json}"
  --o02 "${MARKHAND_O05_O02_REPORT:-bench/markhand_web/reports/phase-1b-gate/o02-alerts.json}"
  --o03 "${MARKHAND_O05_O03_REPORT:-bench/markhand_web/reports/phase-1b-gate/o03-restore.json}"
  --o04 "${MARKHAND_O05_O04_REPORT:-bench/markhand_web/reports/phase-1b-gate/o04-release.json}"
)
if [[ "${MARKHAND_O05_TRUSTED_PREREQUISITES:-0}" == "1" ]]; then
  args+=(--trusted-prerequisite-attestation)
fi
exec python3 bench/markhand_web/soak/run_soak.py \
  "${args[@]}" \
  "$@"
