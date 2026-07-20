#!/usr/bin/env bash
set -euo pipefail

cargo fmt --all -- --check
# Strict Clippy stays on knowledge/server; workspace baseline covers legacy crates.
cargo clippy --no-deps -p fileconv-knowledge -p fileconv-server --all-targets -- -D warnings

if [[ "${RUST_CRATES:-full}" == "full" ]]; then
  python3 scripts/check-rust-lint-baseline.py
  python3 scripts/check-rust-lint-baseline.py --self-test
else
  echo "skipping workspace Clippy baseline for scoped rust gate (${RUST_CRATES})"
fi
