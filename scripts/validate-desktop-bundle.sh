#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUNDLE="$ROOT/target/release/bundle"

test -d "$BUNDLE" || { echo "missing bundle directory: $BUNDLE"; exit 1; }
shopt -s nullglob
debs=("$BUNDLE"/deb/*.deb)
appimages=("$BUNDLE"/appimage/*.AppImage)

if ((${#debs[@]})); then
  dpkg-deb --info "${debs[0]}" | grep -q "Package: markhand"
  dpkg-deb --info "${debs[0]}" | grep -q "Architecture:"
  echo "verified ${debs[0]}"
fi
if ((${#appimages[@]})); then
  file "${appimages[0]}" | grep -Eqi "elf|appimage"
  echo "verified ${appimages[0]}"
fi
if ((${#debs[@]} == 0 && ${#appimages[@]} == 0)); then
  echo "no Linux desktop bundle found"
  exit 1
fi
