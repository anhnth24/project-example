#!/usr/bin/env bash
# CI/local entrypoint: fmt, clippy, and the smallest Rust test gate.
set -euo pipefail
exec bash scripts/run-rust-ci-fast.sh
