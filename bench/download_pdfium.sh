#!/usr/bin/env bash
# Tải thư viện PDFium prebuilt (bblanchon/pdfium-binaries) cho trích text PDF.
# Mặc định linux-x64; đổi PLATFORM cho OS khác (mac-arm64, mac-x64, win-x64...).
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PLATFORM="${1:-linux-x64}"
DST="$ROOT/pdfium"
mkdir -p "$DST"
if [ -f "$DST/lib/libpdfium.so" ] || [ -f "$DST/lib/libpdfium.dylib" ] || [ -f "$DST/bin/pdfium.dll" ]; then
  echo "  đã có pdfium ($PLATFORM)"
else
  echo "  tải pdfium-$PLATFORM …"
  curl -sSL --max-time 180 -o /tmp/pdfium.tgz \
    "https://github.com/bblanchon/pdfium-binaries/releases/latest/download/pdfium-$PLATFORM.tgz"
  tar -xzf /tmp/pdfium.tgz -C "$DST"
fi
echo "  $(cat "$DST/VERSION" 2>/dev/null | tr '\n' ' ')"
echo "Xong pdfium. Backend tự tìm ở ./pdfium/lib (hoặc đặt FILECONV_PDFIUM_LIB)."
