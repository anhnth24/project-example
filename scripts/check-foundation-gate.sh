#!/usr/bin/env bash
set -euo pipefail

required_files=(
  CONTRIBUTING.md
  Makefile
  docs/adr/TEMPLATE.md
  docs/conventions/api.md
  docs/conventions/ci.md
  docs/conventions/config-secrets.md
  docs/conventions/dependencies.md
  docs/conventions/observability-audit.md
  docs/conventions/rust.md
  docs/conventions/sql-migrations.md
  docs/conventions/testing-fixtures.md
  docs/conventions/typescript-react.md
  docs/runbooks/contributor-setup.md
  docs/runbooks/local-development.md
  deploy/dev/compose.yml
)

for path in "${required_files[@]}"; do
  test -f "$path" || {
    echo "foundation gate missing: $path" >&2
    exit 1
  }
done

make check-toolchain
make check-static
cargo metadata --locked --no-deps --format-version 1 >/dev/null
pnpm list --recursive --depth -1 >/dev/null

echo "Phase F foundation files, toolchain and static gates are ready"
