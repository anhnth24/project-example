#!/usr/bin/env bash
# Download pinned promtool (Prometheus 2.55.1) into .tools/ with SHA256 verify.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LOCK="$ROOT/deploy/observability/images.lock.json"
DEST_DIR="$ROOT/.tools"
DEST="$DEST_DIR/promtool"

version="$(python3 -c "import json; print(json.load(open('$LOCK'))['promtool']['version'])")"
asset="$(python3 -c "import json; print(json.load(open('$LOCK'))['promtool']['asset'])")"
sha="$(python3 -c "import json; print(json.load(open('$LOCK'))['promtool']['sha256'])")"
url="$(python3 -c "import json; print(json.load(open('$LOCK'))['promtool']['url'])")"

if [[ -x "$DEST" ]]; then
  if "$DEST" --version 2>&1 | grep -q "version ${version}"; then
    echo "$DEST"
    exit 0
  fi
fi

mkdir -p "$DEST_DIR"
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT
archive="$tmpdir/$asset"
curl -fsSL -o "$archive" "$url"
echo "${sha}  ${archive}" | sha256sum -c -
tar -xzf "$archive" -C "$tmpdir" "prometheus-${version}.linux-amd64/promtool"
install -m 0755 "$tmpdir/prometheus-${version}.linux-amd64/promtool" "$DEST"
"$DEST" --version >/dev/null
echo "$DEST"
