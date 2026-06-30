#!/usr/bin/env bash
# Tải corpus thật (~10 file mỗi loại) phục vụ benchmark backend.
set -u
ROOT="$(cd "$(dirname "$0")" && pwd)"
CORPUS="$ROOT/corpus"
mkdir -p "$CORPUS"/{pdf,docx,pptx,xlsx,csv,html}

dl() { # dl <url> <out>
  curl -sS -L --max-time 60 -o "$2" "$1" 2>/dev/null
  if [ -s "$2" ]; then echo "  ok   $(basename "$2")"; else echo "  FAIL $(basename "$2")"; rm -f "$2"; fi
}

echo "== PDF (arxiv) =="
for id in 1706.03762 1810.04805 1512.03385 1409.1556 1412.6980 1506.02640 1505.04597 1810.13243 2010.11929 1707.06347; do
  dl "https://arxiv.org/pdf/$id" "$CORPUS/pdf/arxiv-$id.pdf"
done

echo "== DOCX (python-docx) =="
DBASE="https://raw.githubusercontent.com/python-openxml/python-docx/master/features/steps/test_files"
for f in blk-containing-table blk-paras-and-tables doc-default doc-coreprops par-known-paragraphs par-hyperlinks hdr-header-footer run-char-style par-alignment doc-odd-even-hdrs; do
  dl "$DBASE/$f.docx" "$CORPUS/docx/$f.docx"
done

echo "== PPTX (python-pptx) =="
PBASE="https://raw.githubusercontent.com/scanny/python-pptx/master/features/steps/test_files"
for f in shp-shapes shp-autoshape-props shp-common-props sld-blank shp-picture shp-groupshape shp-freeform shp-connector-props shp-pos-and-size shp-access-chart; do
  dl "$PBASE/$f.pptx" "$CORPUS/pptx/$f.pptx"
done

echo "== XLSX (PhpSpreadsheet) =="
XBASE="https://raw.githubusercontent.com/PHPOffice/PhpSpreadsheet/master/samples/templates"
for f in 26template 27template 28iterators 31docproperties 32chartreadwrite 21d_FitToHeightPdf 32readwriteBarChart1 32readwriteAreaChart1 32complexChartreadwrite 32readwriteAreaChart2; do
  dl "$XBASE/$f.xlsx" "$CORPUS/xlsx/$f.xlsx"
done

echo "== CSV (burkardt) =="
CBASE="https://people.sc.fsu.edu/~jburkardt/data/csv"
for f in airtravel biostats cities deniro faithful grades homes hw_200 mlb_players snakes_count_10000; do
  dl "$CBASE/$f.csv" "$CORPUS/csv/$f.csv"
done

echo "== HTML (web thật, có cả tiếng Việt) =="
dl "https://example.com/"                                "$CORPUS/html/example.html"
dl "https://www.w3.org/TR/PNG/"                          "$CORPUS/html/w3c-png.html"
dl "https://en.wikipedia.org/wiki/Markdown"              "$CORPUS/html/wiki-markdown.html"
dl "https://en.wikipedia.org/wiki/Rust_(programming_language)" "$CORPUS/html/wiki-rust.html"
dl "https://vi.wikipedia.org/wiki/Vi%E1%BB%87t_Nam"      "$CORPUS/html/wiki-vietnam-vi.html"
dl "https://vi.wikipedia.org/wiki/Ng%C3%B4n_ng%E1%BB%AF_l%E1%BA%adp_tr%C3%ACnh_Rust" "$CORPUS/html/wiki-rust-vi.html"
dl "https://www.rust-lang.org/"                          "$CORPUS/html/rust-lang.html"
dl "https://doc.rust-lang.org/book/title-page.html"      "$CORPUS/html/rust-book.html"
dl "https://httpbin.org/html"                            "$CORPUS/html/httpbin.html"
dl "https://www.gnu.org/licenses/gpl-3.0.en.html"        "$CORPUS/html/gpl3.html"

echo
echo "== Tổng kết số file tải được =="
for d in pdf docx pptx xlsx csv html; do
  n=$(find "$CORPUS/$d" -type f | wc -l)
  echo "  $d: $n file"
done
