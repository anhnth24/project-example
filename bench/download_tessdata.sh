#!/usr/bin/env bash
# Tải model OCR chất lượng cao tessdata_best (vie + eng) để tăng độ chính xác
# tiếng Việt với tài liệu thật (đặc biệt chữ IN HOA). Đặt ở ./tessdata_best —
# backend tự dùng nếu có (hoặc trỏ FILECONV_TESSDATA). Thiếu thì dùng model nhẹ hệ thống.
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DST="$ROOT/tessdata_best"
mkdir -p "$DST"
for l in vie eng; do
  f="$DST/$l.traineddata"
  if [ -s "$f" ]; then
    echo "  đã có $l.traineddata"
  else
    echo "  tải $l.traineddata …"
    curl -sSL --max-time 180 -o "$f" \
      "https://github.com/tesseract-ocr/tessdata_best/raw/main/$l.traineddata"
  fi
  echo "    $(stat -c%s "$f") bytes"
done
echo "Xong. Backend sẽ tự dùng ./tessdata_best cho OCR."
