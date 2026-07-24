#!/usr/bin/env bash
# P1B-O04 vertical-slice / security release suite runner.
# Writes o04-release.json/.md + raw/o04-<git>/ — never O05 summary.json.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

if [[ "${1:-}" == "--self-test" ]]; then
  exec python3 bench/markhand_web/scripts/run_o04_release_suite.py --self-test
fi

echo "O04 release suite (MARKHAND_E2E=${MARKHAND_E2E:-unset})"
exec python3 bench/markhand_web/scripts/run_o04_release_suite.py "$@"
