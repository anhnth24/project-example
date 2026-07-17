#!/usr/bin/env bash
set -euo pipefail

cargo check -p fileconv-knowledge --no-default-features
cargo test -p fileconv-knowledge --no-default-features
cargo check -p fileconv-knowledge --all-features
cargo test -p fileconv-knowledge --all-features

default_tree="$(cargo tree -p fileconv-knowledge --no-default-features -e normal --prefix none)"
if grep -Eq '^(rusqlite|hnsw_rs) ' <<<"$default_tree"; then
  echo "default fileconv-knowledge tree includes desktop adapter dependencies" >&2
  exit 1
fi

all_tree="$(cargo tree -p fileconv-knowledge --all-features -e normal --prefix none)"
grep -Eq '^rusqlite ' <<<"$all_tree"
grep -Eq '^hnsw_rs ' <<<"$all_tree"

python3 scripts/check-architecture-boundaries.py
echo "fileconv-knowledge feature matrix valid"
