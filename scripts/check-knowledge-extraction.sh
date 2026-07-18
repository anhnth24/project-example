#!/usr/bin/env bash
set -euo pipefail

bash scripts/check-knowledge-features.sh
cargo test -p fileconv-server --test knowledge_consumer
cargo test -p fileconv-desktop knowledge_contract
cargo test -p fileconv-desktop knowledge::tests
python3 scripts/check-architecture-boundaries.py
python3 scripts/check-fixtures.py --root crates/knowledge/fixtures
python3 scripts/build-roadmap.py --check

echo "Phase 1A knowledge extraction gate passed"
