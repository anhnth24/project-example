#!/usr/bin/env bash
# Render ảnh tiếng Việt từ ref.txt để kiểm thử OCR.
# Cần ImageMagick + font DejaVu. Font viết tay tải từ Google Fonts (có subset Vietnamese).
set -eu
DIR="$(cd "$(dirname "$0")" && pwd)/vn_corpus"
cd "$DIR"
TEXT="$(cat ref.txt)"
DEJAVU=/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf

# Ảnh chữ in rõ nét (kịch bản OCR chuẩn / text-as-image trong slide)
convert -background white -fill black -font "$DEJAVU" -pointsize 30 -size 1200x \
  caption:"$TEXT" -bordercolor white -border 20 vn_printed.png
# Ảnh phân giải thấp (mô phỏng scan kém)
convert -background white -fill black -font "$DEJAVU" -pointsize 16 -size 620x \
  caption:"$TEXT" -bordercolor white -border 8 vn_lowres.png

# Font "viết tay" (mô phỏng — KHÔNG phải chữ viết tay người thật)
mkdir -p fonts
[ -f fonts/Caveat.ttf ] || curl -sSL -o fonts/Caveat.ttf \
  "https://github.com/google/fonts/raw/main/ofl/caveat/Caveat%5Bwght%5D.ttf"
[ -f fonts/DancingScript.ttf ] || curl -sSL -o fonts/DancingScript.ttf \
  "https://github.com/google/fonts/raw/main/ofl/dancingscript/DancingScript%5Bwght%5D.ttf"
convert -background white -fill black -font fonts/Caveat.ttf -pointsize 40 -size 1200x \
  caption:"$TEXT" -bordercolor white -border 20 vn_hand_caveat.png
convert -background white -fill black -font fonts/DancingScript.ttf -pointsize 38 -size 1300x \
  caption:"$TEXT" -bordercolor white -border 20 vn_hand_dancing.png

echo "Đã render: vn_printed.png vn_lowres.png vn_hand_caveat.png vn_hand_dancing.png"
