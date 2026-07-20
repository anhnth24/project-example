#!/usr/bin/env bash
# Run Rust tests for the crate scopes selected by classify-ci-changes.py.
set -euo pipefail

if [[ $# -ne 1 || -z "${1// /}" ]]; then
  echo "usage: $0 <comma-separated-crate-scopes>" >&2
  echo "scopes: core cli desktop knowledge server mcp" >&2
  exit 2
fi

IFS=',' read -r -a SCOPES <<<"$1"
declare -A SEEN=()
ordered=()
for scope in "${SCOPES[@]}"; do
  scope="${scope// /}"
  if [[ -z "$scope" ]]; then
    continue
  fi
  case "$scope" in
    core | cli | desktop | knowledge | server | mcp) ;;
    *)
      echo "unknown rust crate scope: $scope" >&2
      exit 2
      ;;
  esac
  if [[ -z "${SEEN[$scope]+x}" ]]; then
    SEEN["$scope"]=1
    ordered+=("$scope")
  fi
done

if ((${#ordered[@]} == 0)); then
  echo "no rust crate scopes to test" >&2
  exit 2
fi

echo "running scoped rust tests: ${ordered[*]}"

for scope in "${ordered[@]}"; do
  case "$scope" in
    core)
      cargo test -p fileconv-core
      cargo test -p fileconv-core --features llm llm
      ;;
    cli)
      cargo test -p fileconv-cli metrics
      ;;
    desktop)
      cargo test -p fileconv-desktop
      ;;
    knowledge)
      bash scripts/check-knowledge-features.sh
      cargo test -p fileconv-knowledge --all-features
      ;;
    server)
      # One compile graph; lib tests give fast PR signal. Integration tests run on full gate.
      cargo test -p fileconv-knowledge --no-default-features -p fileconv-server --lib
      ;;
    mcp)
      cargo test -p fileconv-mcp
      ;;
  esac
done

echo "scoped rust tests passed: ${ordered[*]}"
