#!/usr/bin/env bash
set -euo pipefail

cargo fmt --all -- --check
cargo clippy --no-deps -p fileconv-knowledge -p fileconv-server --all-targets -- -D warnings
python3 scripts/check-rust-lint-baseline.py
python3 scripts/check-rust-lint-baseline.py --self-test
