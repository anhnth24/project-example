#!/usr/bin/env bash
# Tải model whisper GGML (tiny/base/small) cho phiên âm audio.
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$ROOT/models"
B="https://huggingface.co/ggerganov/whisper.cpp/resolve/main"
for m in tiny base small; do
  f="$ROOT/models/ggml-$m.bin"
  if [ -s "$f" ]; then
    echo "  đã có ggml-$m.bin"
  else
    echo "  tải ggml-$m.bin …"
    curl -sSL --max-time 600 -o "$f" "$B/ggml-$m.bin"
  fi
  echo "    $(stat -c%s "$f") bytes"
done
echo "Xong model."
