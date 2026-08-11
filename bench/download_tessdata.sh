#!/usr/bin/env bash
# [LỊCH SỬ — ADR 0016] Tesseract đã bị loại bỏ khỏi pipeline OCR (2026-08-10);
# backend không còn đọc tessdata/FILECONV_TESSDATA. Script này chỉ giữ để tái
# tạo thí nghiệm OCR cũ (bench/REPORT_ACCURACY.md, ocr_experiment.py).
# Tải model tessdata_best (vie + eng) vào ./tessdata_best.
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
