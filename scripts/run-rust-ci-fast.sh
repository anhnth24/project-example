#!/usr/bin/env bash
# Single Rust CI step: fmt, strict clippy, then tests with one shared build graph.
set -euo pipefail

RUST_CRATES="${RUST_CRATES:-full}"
KNOWLEDGE_GATE="${KNOWLEDGE_GATE:-false}"
INTEGRATION="${RUST_INTEGRATION:-false}"

cargo fmt --all -- --check
cargo clippy --no-deps -p fileconv-knowledge -p fileconv-server --all-targets -- -D warnings

if [[ "$RUST_CRATES" == "full" ]]; then
  python3 scripts/check-rust-lint-baseline.py
  python3 scripts/check-rust-lint-baseline.py --self-test
fi

if [[ "$KNOWLEDGE_GATE" == "true" ]]; then
  make check-knowledge-extraction-rust
elif [[ "$RUST_CRATES" == "full" || "$INTEGRATION" == "true" ]]; then
  make check-rust-tests
else
  bash scripts/check-rust-tests-scoped.sh "$RUST_CRATES"
fi
