#!/usr/bin/env bash
# CI/local entrypoint: pick the smallest Rust test gate for the changed paths.
set -euo pipefail

KNOWLEDGE_GATE="${KNOWLEDGE_GATE:-false}"
RUST_CRATES="${RUST_CRATES:-full}"

if [[ "$KNOWLEDGE_GATE" == "true" ]]; then
  make check-knowledge-extraction-rust
elif [[ "$RUST_CRATES" == "full" ]]; then
  make check-rust-tests
else
  bash scripts/check-rust-tests-scoped.sh "$RUST_CRATES"
fi
