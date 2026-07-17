#!/usr/bin/env bash
set -euo pipefail

required_node_major=20
required_pnpm_version=10.33.3

node_major="$(node --version | sed -E 's/^v([0-9]+).*/\1/')"
if [[ "$node_major" -lt "$required_node_major" ]]; then
  echo "Node.js >= ${required_node_major} required; found $(node --version)" >&2
  exit 1
fi

pnpm_version="$(pnpm --version)"
if [[ "$pnpm_version" != "$required_pnpm_version" ]]; then
  echo "pnpm ${required_pnpm_version} required; found ${pnpm_version}" >&2
  exit 1
fi

test -f deploy/dev/compose.yml || {
  echo "missing deploy/dev/compose.yml" >&2
  exit 1
}
if command -v docker >/dev/null; then
  docker compose version
else
  echo "Docker Compose is required for dev-* targets (not installed in this shell)"
fi

echo "web toolchain: node $(node --version), pnpm ${pnpm_version}"
