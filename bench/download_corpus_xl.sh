#!/usr/bin/env bash
# Tải corpus MỞ RỘNG (~15-20 file/loại) + bộ edge-case để soi lỗi.
set -u
ROOT="$(cd "$(dirname "$0")" && pwd)"
C="$ROOT/corpus_xl"
E="$ROOT/edge"
mkdir -p "$C"/{pdf,docx,pptx,xlsx,csv,html} "$E"
dl(){ curl -sS -L --max-time 90 -o "$2" "$1" 2>/dev/null; if [ -s "$2" ]; then echo "ok $(basename "$2")"; else echo "FAIL $(basename "$2")"; rm -f "$2"; fi; }

echo "===== PDF (arxiv đa dạng + sample) ====="
for id in 1706.03762 1810.04805 1512.03385 1409.1556 1412.6980 1506.02640 1505.04597 1810.13243 2010.11929 1707.06347 1301.3781 1502.03167 1611.07004 2005.14165 1606.08415; do
  dl "https://arxiv.org/pdf/$id" "$C/pdf/arxiv-$id.pdf"; done
dl "https://africau.edu/images/default/sample.pdf" "$C/pdf/africau-sample.pdf"
dl "https://www.w3.org/WAI/ER/tests/xhtml/testfiles/resources/pdf/dummy.pdf" "$C/pdf/w3c-dummy.pdf"

echo "===== DOCX (python-docx) ====="
DB="https://raw.githubusercontent.com/python-openxml/python-docx/master/features/steps/test_files"
for f in blk-containing-table blk-paras-and-tables comments-rich-para doc-access-sections doc-add-section doc-coreprops doc-default doc-odd-even-hdrs fnt-color hdr-header-footer num-having-numbering-part par-alignment par-hlink-frags par-hyperlinks par-known-paragraphs par-known-styles par-rendered-page-breaks run-char-style sct-inner-content; do
  dl "$DB/$f.docx" "$C/docx/$f.docx"; done

echo "===== PPTX (python-pptx) ====="
PB="https://raw.githubusercontent.com/scanny/python-pptx/master/features/steps/test_files"
for f in cht-charts cht-legend dml-fill dml-line ext-rels font-color lyt-shapes minimal mst-placeholders mst-shapes shp-shapes shp-autoshape-props shp-common-props shp-groupshape shp-picture shp-pos-and-size shp-freeform shp-connector-props sld-blank shp-access-chart; do
  dl "$PB/$f.pptx" "$C/pptx/$f.pptx"; done

echo "===== XLSX (PhpSpreadsheet) ====="
XB="https://raw.githubusercontent.com/PHPOffice/PhpSpreadsheet/master/samples/templates"
for f in 21d_FitToHeightPdf 26template 27template 28iterators 31docproperties 32chartreadwrite 32complexChartreadwrite 32readwriteAreaChart1 32readwriteBarChart1 32readwriteBubbleChart1 32readwriteLineChart1 32readwritePieChart1 32readwriteScatterChart1 32readwriteStockChart1 33thumbnail excelExplorer GnumericTest OOCalcTest SylkTest old.xls; do
  dl "$XB/$f.xlsx" "$C/xlsx/$f.xlsx"; done

echo "===== CSV (burkardt + datasets) ====="
CB="https://people.sc.fsu.edu/~jburkardt/data/csv"
for f in airtravel biostats cities deniro faithful grades homes hw_200 hw_25000 mlb_players news_decline nile oscar_age_female oscar_age_male snakes_count_10000 taxables trees zillow letter_frequency ford_escort; do
  dl "$CB/$f.csv" "$C/csv/$f.csv"; done

echo "===== HTML (web thật đa ngôn ngữ/đa dạng) ====="
declare -A H=(
 [example]=https://example.com/
 [w3c-png]=https://www.w3.org/TR/PNG/
 [wiki-md-en]=https://en.wikipedia.org/wiki/Markdown
 [wiki-rust-en]=https://en.wikipedia.org/wiki/Rust_(programming_language)
 [wiki-vn-vi]=https://vi.wikipedia.org/wiki/Vi%E1%BB%87t_Nam
 [wiki-rust-vi]=https://vi.wikipedia.org/wiki/Ng%C3%B4n_ng%E1%BB%AF_l%E1%BA%adp_tr%C3%ACnh_Rust
 [wiki-jp]=https://ja.wikipedia.org/wiki/Rust
 [wiki-ar]=https://ar.wikipedia.org/wiki/%D8%B1%D8%B3%D8%AA_(%D9%84%D8%BA%D8%A9_%D8%A8%D8%B1%D9%85%D8%AC%D8%A9)
 [rust-lang]=https://www.rust-lang.org/
 [rust-book]=https://doc.rust-lang.org/book/title-page.html
 [httpbin]=https://httpbin.org/html
 [gpl3]=https://www.gnu.org/licenses/gpl-3.0.en.html
 [mdn]=https://developer.mozilla.org/en-US/docs/Web/HTML
 [news-hn]=https://news.ycombinator.com/
 [python-docs]=https://docs.python.org/3/tutorial/index.html
 [w3-html]=https://www.w3.org/TR/html52/
 [iana]=https://www.iana.org/
 [rfc-ed]=https://www.rfc-editor.org/
)
for k in "${!H[@]}"; do dl "${H[$k]}" "$C/html/$k.html"; done

echo
echo "===== EDGE CASES ====="
# 1. file rỗng
: > "$E/empty.pdf"; : > "$E/empty.docx"; : > "$E/empty.csv"; echo "tạo empty.*"
# 2. file hỏng / rác
head -c 2000 /dev/urandom > "$E/corrupt.pdf"; echo "tạo corrupt.pdf"
head -c 3000 /dev/urandom > "$E/corrupt.docx"; echo "tạo corrupt.docx (zip hỏng)"
# 3. sai đuôi: html giả danh pdf; text giả danh docx
cp "$C/html/example.html" "$E/actually-html.pdf" 2>/dev/null && echo "tạo actually-html.pdf"
printf 'xin chào, đây là text thường\n' > "$E/actually-text.docx"; echo "tạo actually-text.docx"
# 4. legacy binary (không hỗ trợ) — tải .doc/.xls/.ppt thật
dl "https://file-examples.com/wp-content/storage/2017/02/file-sample_100kB.doc" "$E/legacy.doc"
dl "https://file-examples.com/wp-content/storage/2017/02/file_example_XLS_10.xls" "$E/legacy.xls"
dl "https://file-examples.com/wp-content/storage/2017/08/file_example_PPT_250kB.ppt" "$E/legacy.ppt"
# 5. CSV unicode/dấu phẩy trong ô/nhiều dòng
printf 'Tên,Ghi chú,Số tiền\n"Nguyễn, Văn A","Dòng 1\nDòng 2",1.000.000\n"日本語","emoji 😀🎉","-5"\n' > "$E/tricky.csv"; echo "tạo tricky.csv"
# 6. CSV chỉ 1 cột / 1 ô
printf 'chỉ một ô\n' > "$E/single.csv"; echo "tạo single.csv"
# 7. HTML rỗng tag / chỉ script
printf '<html><body><script>alert(1)</script><style>p{}</style></body></html>' > "$E/only-script.html"; echo "tạo only-script.html"

echo
echo "===== TỔNG KẾT ====="
for d in "$C"/*/; do echo "  $(basename "$d"): $(find "$d" -type f | wc -l) file"; done
echo "  edge: $(find "$E" -type f | wc -l) file"
