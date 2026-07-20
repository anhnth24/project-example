#!/usr/bin/env bash
set -euo pipefail

RUST_CRATES="${RUST_CRATES:-knowledge,server}"

declare -A CLIPPY_PACKAGES=()
add_package() {
  CLIPPY_PACKAGES["$1"]=1
}

if [[ "$RUST_CRATES" == "full" ]]; then
  for package in fileconv-core fileconv-knowledge fileconv-server fileconv-cli fileconv-desktop fileconv-mcp; do
    add_package "$package"
  done
else
  IFS=',' read -r -a SCOPES <<<"$RUST_CRATES"
  for scope in "${SCOPES[@]}"; do
    scope="${scope// /}"
    case "$scope" in
      core) add_package fileconv-core ;;
      knowledge) add_package fileconv-knowledge ;;
      server)
        add_package fileconv-knowledge
        add_package fileconv-server
        ;;
      cli) add_package fileconv-cli ;;
      desktop) add_package fileconv-desktop ;;
      mcp) add_package fileconv-mcp ;;
      *)
        echo "unknown rust crate scope for clippy: $scope" >&2
        exit 2
        ;;
    esac
  done
fi

if ((${#CLIPPY_PACKAGES[@]} == 0)); then
  add_package fileconv-knowledge
  add_package fileconv-server
fi

args=()
for package in "${!CLIPPY_PACKAGES[@]}"; do
  args+=(-p "$package")
done

cargo fmt --all -- --check
cargo clippy --no-deps "${args[@]}" --all-targets -- -D warnings
python3 scripts/check-rust-lint-baseline.py
python3 scripts/check-rust-lint-baseline.py --self-test
