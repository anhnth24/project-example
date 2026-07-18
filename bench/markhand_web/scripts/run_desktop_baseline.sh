#!/usr/bin/env bash
set -euo pipefail

cargo build --release -p fileconv-cli
python3 bench/markhand_web/scripts/run_desktop_baseline.py "$@"
