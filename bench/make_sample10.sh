#!/usr/bin/env bash
# Dựng bộ SAMPLE10: 10 loại tài liệu × 5 file, có KIỂM TRA định dạng (magic byte).
# File hỏng bị loại và thay bằng nguồn khác. Dùng cho test báo cáo.
set -u
ROOT="$(cd "$(dirname "$0")" && pwd)"
S="$ROOT/sample10"
mkdir -p "$S"/{pdf,docx,pptx,xlsx,xls,csv,html,image_png,image_other,audio}

dl(){ curl -sS -L --max-time 90 -o "$2" "$1" 2>/dev/null; }
ok_pdf(){ head -c5 "$1" 2>/dev/null | grep -q "%PDF"; }
ok_zip(){ head -c2 "$1" 2>/dev/null | grep -q "PK"; }
ok_ole(){ [ "$(head -c4 "$1" 2>/dev/null | od -An -tx1 | tr -d ' \n')" = "d0cf11e0" ]; }
keep(){ if $2 "$1"; then echo "  ok   $(basename "$1")"; else echo "  LOẠI $(basename "$1")"; rm -f "$1"; fi; }

echo "== 1. PDF (arxiv) =="
for id in 1706.03762 1512.03385 1409.1556 1301.3781 1810.04805; do
  f="$S/pdf/arxiv-$id.pdf"; [ -s "$f" ] || dl "https://arxiv.org/pdf/$id" "$f"; keep "$f" ok_pdf
done

echo "== 2. DOCX =="
DB="https://raw.githubusercontent.com/python-openxml/python-docx/master/features/steps/test_files"
for f in blk-containing-table doc-default par-known-paragraphs hdr-header-footer; do
  o="$S/docx/$f.docx"; [ -s "$o" ] || dl "$DB/$f.docx" "$o"; keep "$o" ok_zip
done
o="$S/docx/calibre-demo.docx"; [ -s "$o" ] || dl "https://calibre-ebook.com/downloads/demos/demo.docx" "$o"; keep "$o" ok_zip

echo "== 3. PPTX =="
PB="https://raw.githubusercontent.com/scanny/python-pptx/master/features/steps/test_files"
for f in shp-shapes mst-placeholders lyt-shapes cht-charts shp-groupshape; do
  o="$S/pptx/$f.pptx"; [ -s "$o" ] || dl "$PB/$f.pptx" "$o"; keep "$o" ok_zip
done

echo "== 4. XLSX =="
XB="https://raw.githubusercontent.com/PHPOffice/PhpSpreadsheet/master/samples/templates"
for f in 26template 28iterators 31docproperties 32chartreadwrite 21d_FitToHeightPdf; do
  o="$S/xlsx/$f.xlsx"; [ -s "$o" ] || dl "$XB/$f.xlsx" "$o"; keep "$o" ok_zip
done

echo "== 5. XLS legacy (BIFF) =="
# 1 file thật từ PhpSpreadsheet + 4 file sinh bằng xlwt (BIFF thật, nội dung tiếng Việt)
o="$S/xls/phpspreadsheet-sample.xls"; [ -s "$o" ] || dl "https://github.com/PHPOffice/PhpSpreadsheet/raw/master/tests/data/Reader/XLS/sample.xls" "$o"; keep "$o" ok_ole
python3 - "$S/xls" <<'PY'
import sys, xlwt
d = sys.argv[1]
datasets = {
  "hoadon":    [["Số HĐ","Ngày","Khách hàng","Tổng tiền"],["HD001","01/06/2026","Nguyễn Văn A",1200000],["HD002","02/06/2026","Trần Thị Bình",560000]],
  "danhsach":  [["STT","Họ tên","Đơn vị"],[1,"Lê Hoàng Cường","Phòng PC06"],[2,"Phạm Thu Dung","Công an phường"]],
  "baocao":    [["Chỉ tiêu","Quý 1","Quý 2"],["Doanh thu (tỷ)",12.5,13.8],["Chi phí (tỷ)",7.2,8.1]],
  "diemthi":   [["Mã SV","Toán","Văn"],["SV01",8.5,7.0],["SV02",9.0,8.25]],
}
for name, rows in datasets.items():
    wb = xlwt.Workbook(encoding="utf-8"); ws = wb.add_sheet("Sheet1")
    for r, row in enumerate(rows):
        for c, v in enumerate(row): ws.write(r, c, v)
    wb.save(f"{d}/vn-{name}.xls"); print(f"  ok   vn-{name}.xls (xlwt)")
PY

echo "== 6. CSV =="
CB="https://people.sc.fsu.edu/~jburkardt/data/csv"
for f in airtravel biostats cities grades homes; do
  o="$S/csv/$f.csv"; [ -s "$o" ] || dl "$CB/$f.csv" "$o"
  [ -s "$o" ] && echo "  ok   $f.csv" || echo "  LOẠI $f.csv"
done

echo "== 7. HTML =="
while IFS='|' read -r name url; do [ -z "$name" ] && continue
  o="$S/html/$name.html"; [ -s "$o" ] || dl "$url" "$o"
  [ -s "$o" ] && echo "  ok   $name.html" || echo "  LOẠI $name.html"
done <<'EOF'
example|https://example.com/
wiki-vn|https://vi.wikipedia.org/wiki/Việt_Nam
rust-book|https://doc.rust-lang.org/book/title-page.html
gpl3|https://www.gnu.org/licenses/gpl-3.0.en.html
httpbin|https://httpbin.org/html
EOF

echo "== 8. IMAGE PNG (render tiếng Việt, kèm .gt.txt) =="
SANS=/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf
SERIF=/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf
gen(){ convert -background white -fill black -font "$2" -pointsize "$3" -size "$4"x caption:"$5" -bordercolor white -border 15 "$S/image_png/$1" 2>/dev/null && echo "  ok   $1"; printf '%s' "$5" > "$S/image_png/${1%.png}.gt.txt"; }
gen vn_thongbao.png "$SANS" 28 1000 "Thông báo: Cuộc họp giao ban tháng 6 năm 2026 sẽ diễn ra vào lúc 08 giờ 30 phút ngày thứ Sáu."
gen vn_hopdong.png "$SERIF" 26 1000 "Hợp đồng số 123/2026/HĐ-KT được ký giữa Công ty TNHH ABC và Sở Tư pháp Hà Nội, giá trị 1.234.567.890 đồng."
gen vn_diachi.png "$SANS" 24 900 "Trụ sở: số 58 - 60 Trần Phú, quận Ba Đình, thành phố Hà Nội. Điện thoại: 024.62739655."
gen vn_thongke.png "$SERIF" 24 950 "Thống kê quý 1: tổng số 1.245 hồ sơ, đã xử lý 1.180 hồ sơ, đạt tỷ lệ 94,8 phần trăm."
gen vn_quyet.png "$SANS" 26 1000 "Quyết định về việc ban hành quy chế làm việc của Ủy ban nhân dân thành phố, có hiệu lực từ ngày 01 tháng 7 năm 2026."

echo "== 9. IMAGE khác định dạng (jpg/webp/bmp/tiff/gif từ PNG trên) =="
i=0
for fmt in jpg webp bmp tiff gif; do
  i=$((i+1)); src=$(ls "$S/image_png"/*.png | sed -n "${i}p")
  o="$S/image_other/$(basename "${src%.png}").$fmt"
  convert "$src" "$o" 2>/dev/null && echo "  ok   $(basename "$o")"
  cp "${src%.png}.gt.txt" "${o%.*}.gt.txt" 2>/dev/null
done

echo "== 10. AUDIO (gTTS tiếng Việt + 1 EN, kèm .gt.txt) =="
python3 - "$S/audio" <<'PY'
import sys, os
from gtts import gTTS
d = sys.argv[1]
clips = {
 "vn_giaoban": ("vi", "Cuộc họp giao ban sáng thứ hai bắt đầu lúc tám giờ ba mươi phút."),
 "vn_baocao":  ("vi", "Báo cáo tổng kết quý một đã được gửi tới toàn thể các đơn vị."),
 "vn_sodienthoai": ("vi", "Vui lòng liên hệ số điện thoại không hai bốn, sáu hai bảy ba chín sáu năm năm."),
 "vn_camon":   ("vi", "Trân trọng cảm ơn sự phối hợp của quý cơ quan và đơn vị."),
 "en_test":    ("en", "This is a short English test sentence for speech recognition."),
}
for name, (lang, text) in clips.items():
    p = f"{d}/{name}.mp3"
    if not os.path.exists(p):
        gTTS(text=text, lang=lang).save(p)
    open(f"{d}/{name}.gt.txt", "w", encoding="utf-8").write(text)
    print(f"  ok   {name}.mp3")
PY

echo
echo "== TỔNG KẾT =="
for d in pdf docx pptx xlsx xls csv html image_png image_other audio; do
  n=$(find "$S/$d" -type f ! -name "*.gt.txt" | wc -l)
  echo "  $d: $n file"
done
