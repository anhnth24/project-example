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
exec python3 bench/markhand_web/soak/run_soak.py \
  --profile bench/markhand_web/workloads/phase1b-mixed.yaml \
  --out bench/markhand_web/reports/phase-1b-gate \
  "$@"
