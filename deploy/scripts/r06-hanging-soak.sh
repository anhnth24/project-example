#!/usr/bin/env bash
# P1B-R06 hanging-dependency Compose soak runner.
# Writes bench/markhand_web/reports/phase-1b-gate/r06-hanging-soak.{json,md}
# + raw/r06-<stamp>/. Mirrors the deploy/scripts/o05-soak.sh wrapper style.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

if [[ "${1:-}" == "--self-test" ]]; then
  exec python3 bench/markhand_web/hanging_soak/run_hanging_soak.py --self-test
fi

echo "R06 hanging-dependency soak (MARKHAND_HANGING_SOAK=${MARKHAND_HANGING_SOAK:-unset})"
exec python3 bench/markhand_web/hanging_soak/run_hanging_soak.py \
  --out "${MARKHAND_R06_OUT_DIR:-bench/markhand_web/reports/phase-1b-gate}" \
  "$@"
