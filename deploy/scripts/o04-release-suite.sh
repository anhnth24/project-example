#!/usr/bin/env bash
# P1B-O04 vertical-slice / security release suite runner.
# Writes o04-release.json/.md + raw/o04-<git>/ — never O05 summary.json.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

if [[ "${1:-}" == "--self-test" ]]; then
  exec python3 bench/markhand_web/scripts/run_o04_release_suite.py --self-test
fi

if [[ "${1:-}" == "--release-gate" ]]; then
  shift
  : "${GITHUB_SHA:=$(git rev-parse HEAD)}"
  export GITHUB_SHA
  out="${O04_OUTPUT_DIR:-${TMPDIR:-/tmp}/markhand-o04-release-${GITHUB_SHA}}"
  rm -rf "$out"
  mkdir -p "$out"
  echo "O04 release gate (generate outside source tree, then validate canonical pass)"
  python3 bench/markhand_web/scripts/run_o04_release_suite.py --output-dir "$out"
  exec env MARKHAND_RELEASE_GATE=1 O04_REPORT_PATH="$out/o04-release.json" \
    cargo test -p fileconv-server --test e2e_release_suite -- "$@"
fi

echo "O04 release suite (MARKHAND_E2E=${MARKHAND_E2E:-unset})"
exec python3 bench/markhand_web/scripts/run_o04_release_suite.py "$@"
