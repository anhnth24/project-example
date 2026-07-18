#!/usr/bin/env bash
set -euo pipefail

cargo check -p fileconv-knowledge --no-default-features
cargo test -p fileconv-knowledge --no-default-features
cargo check -p fileconv-knowledge --no-default-features --features desktop-sqlite
cargo check -p fileconv-knowledge --no-default-features --features desktop-hnsw
cargo check -p fileconv-knowledge --all-features
cargo test -p fileconv-knowledge --all-features

default_tree="$(cargo tree -p fileconv-knowledge --no-default-features -e normal --prefix none)"
if grep -Eq '^(fs2|rusqlite|hnsw_rs) ' <<<"$default_tree"; then
  echo "default fileconv-knowledge tree includes desktop adapter dependencies" >&2
  exit 1
fi

all_tree="$(cargo tree -p fileconv-knowledge --all-features -e normal --prefix none)"
grep -Eq '^fs2 ' <<<"$all_tree"
grep -Eq '^rusqlite ' <<<"$all_tree"
grep -Eq '^hnsw_rs ' <<<"$all_tree"

server_tree="$(cargo tree -p fileconv-server -e normal --prefix none)"
if grep -Eq '^(fs2|hnsw_rs|rusqlite|tauri) ' <<<"$server_tree"; then
  echo "fileconv-server default tree includes desktop dependencies" >&2
  exit 1
fi

python3 scripts/check-architecture-boundaries.py
echo "fileconv-knowledge feature matrix valid"
