#!/usr/bin/env bash
# Workspace Rust tests without the desktop crate (no GTK required).
set -euo pipefail

INTEGRATION="${RUST_INTEGRATION:-false}"
SERVER_TEST=(--lib)
if [[ "$INTEGRATION" == "true" ]]; then
  SERVER_TEST=()
fi

cargo test -p fileconv-core
cargo test -p fileconv-core --features llm llm
cargo test -p fileconv-cli metrics
cargo test -p fileconv-knowledge --no-default-features -p fileconv-server "${SERVER_TEST[@]}"
cargo test -p fileconv-mcp

echo "workspace rust tests passed (integration=${INTEGRATION})"
