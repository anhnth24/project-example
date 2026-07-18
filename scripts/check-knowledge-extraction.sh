#!/usr/bin/env bash
set -euo pipefail

rust_only=false
if [[ "${1:-}" == "--rust-only" ]]; then
  rust_only=true
elif [[ $# -ne 0 ]]; then
  echo "usage: $0 [--rust-only]" >&2
  exit 2
fi

bash scripts/check-knowledge-features.sh
cargo test -p fileconv-core
cargo test -p fileconv-core --features llm llm
cargo test -p fileconv-cli metrics
cargo test -p fileconv-server
cargo test -p fileconv-desktop
python3 scripts/check-architecture-boundaries.py
make check-fixtures
python3 scripts/build-roadmap.py --check

if [[ "$rust_only" == false ]]; then
  pnpm --filter markhand-desktop test
  pnpm --filter markhand-desktop build
fi

echo "Phase 1A knowledge extraction gate passed"
