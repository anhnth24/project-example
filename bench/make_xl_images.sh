#!/usr/bin/env bash
# Sinh ảnh test đa dạng: font, kiểu chữ, kích thước, định dạng. Lưu kèm ground-truth.
set -u
ROOT="$(cd "$(dirname "$0")" && pwd)"
D="$ROOT/corpus_xl/image"; mkdir -p "$D"
SANS=/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf
SERIF=/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf
FREEB=/usr/share/fonts/truetype/freefont/FreeSansBold.ttf
HAND="$ROOT/vn_corpus/fonts/Caveat.ttf"

VN="Cộng hòa Xã hội Chủ nghĩa Việt Nam. Hôm nay là ngày 30 tháng 6 năm 2026. Tôi đang kiểm thử công cụ OCR tiếng Việt."
VNCAPS="CỘNG HÒA XÃ HỘI CHỦ NGHĨA VIỆT NAM ĐỘC LẬP TỰ DO HẠNH PHÚC GIẤY MỜI HỌP"
EN="The quick brown fox jumps over the lazy dog. Pack my box with five dozen liquor jugs."
MIX="Hợp đồng số 123/2026/HĐ-KT, giá trị 1.234.567 đồng (≈ \$50). Email: test@moj.gov.vn"

gen(){ # gen <file> <font> <pointsize> <width> <text>
  convert -background white -fill black -font "$2" -pointsize "$3" -size "$4"x caption:"$5" -bordercolor white -border 15 "$D/$1" 2>/dev/null && echo "ok $1" || echo "FAIL $1"
}
gen vn_sans.png   "$SANS"  30 1100 "$VN";    echo "$VN"    > "$D/vn_sans.gt.txt"
gen vn_serif.png  "$SERIF" 30 1100 "$VN";    echo "$VN"    > "$D/vn_serif.gt.txt"
gen vn_caps.png   "$FREEB" 30 1100 "$VNCAPS"; echo "$VNCAPS" > "$D/vn_caps.gt.txt"
gen vn_tiny.png   "$SANS"  14 600  "$VN";    echo "$VN"    > "$D/vn_tiny.gt.txt"
gen vn_hand.png   "$HAND"  40 1100 "$VN";    echo "$VN"    > "$D/vn_hand.gt.txt"
gen en_text.png   "$SERIF" 28 1000 "$EN";    echo "$EN"    > "$D/en_text.gt.txt"
gen mixed.png     "$SANS"  26 1100 "$MIX";   echo "$MIX"   > "$D/mixed.gt.txt"

# Đa định dạng từ cùng 1 ảnh VN (kiểm tra decode jpg/webp/bmp/tiff/gif)
for fmt in jpg webp bmp tiff gif; do
  convert "$D/vn_sans.png" "$D/vn_sans.$fmt" 2>/dev/null && echo "ok vn_sans.$fmt" || echo "FAIL vn_sans.$fmt"
done

# Ảnh KHÔNG có chữ (OCR nên ra rỗng) — vẽ hình khối
convert -size 400x300 xc:white -fill blue -draw "circle 200,150 200,50" "$D/no_text.png" 2>/dev/null && echo "ok no_text.png"

echo "Tổng ảnh: $(find "$D" -type f \( -name '*.png' -o -name '*.jpg' -o -name '*.webp' -o -name '*.bmp' -o -name '*.tiff' -o -name '*.gif' \) | wc -l)"
