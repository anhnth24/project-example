#!/usr/bin/env bash
# Single Rust CI step: fmt, strict clippy, then tests with one shared build graph.
set -euo pipefail

RUST_CRATES="${RUST_CRATES:-full}"
KNOWLEDGE_GATE="${KNOWLEDGE_GATE:-false}"
INTEGRATION="${RUST_INTEGRATION:-false}"

cargo fmt --all -- --check

clippy_args=(--lib)
if [[ "$RUST_CRATES" == "full" || "$INTEGRATION" == "true" ]]; then
  clippy_args=(--all-targets)
fi
cargo clippy --no-deps -p fileconv-knowledge -p fileconv-server "${clippy_args[@]}" -- -D warnings

if [[ "$RUST_CRATES" == "full" ]]; then
  python3 scripts/check-rust-lint-baseline.py
  python3 scripts/check-rust-lint-baseline.py --self-test
fi

if [[ "$KNOWLEDGE_GATE" == "true" && "$RUST_CRATES" != "smoke" && "$RUST_CRATES" != "workspace" ]]; then
  make check-knowledge-extraction-rust
elif [[ "$RUST_CRATES" == "smoke" ]]; then
  if [[ "$INTEGRATION" == "true" ]]; then
    bash scripts/check-rust-tests-workspace.sh
  else
    cargo test -p fileconv-knowledge --no-default-features -p fileconv-server --lib
  fi
elif [[ "$RUST_CRATES" == "workspace" ]]; then
  bash scripts/check-rust-tests-workspace.sh
elif [[ "$RUST_CRATES" == "full" ]]; then
  make check-rust-tests
else
  bash scripts/check-rust-tests-scoped.sh "$RUST_CRATES"
fi
