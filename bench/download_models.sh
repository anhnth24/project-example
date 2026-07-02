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

# PhoWhisper (VinAI, fine-tune 844h tiếng Việt) — bản ggml cộng đồng (dongxiat).
# Đo thật trên corpus vi: 90.8% vs 77.3% của whisper-small cùng cỡ (+13.5 điểm).
# LƯU Ý license: repo PhoWhisper không ghi license rõ — kiểm tra trước khi phân phối thương mại.
f="$ROOT/models/ggml-PhoWhisper-small.bin"
if [ -s "$f" ]; then echo "  đã có ggml-PhoWhisper-small.bin"; else
  echo "  tải ggml-PhoWhisper-small.bin …"
  curl -sSL --max-time 900 -o "$f" "https://huggingface.co/dongxiat/ggml-PhoWhisper-small/resolve/main/ggml-PhoWhisper-small.bin"
fi
echo "    $(stat -c%s "$f") bytes"
