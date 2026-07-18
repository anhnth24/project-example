#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$ROOT"

# Prefer local snapshot/native assets when present (not committed).
if [[ -z "${FILECONV_PDFIUM_LIB:-}" && -d "$ROOT/pdfium/lib" ]]; then
  export FILECONV_PDFIUM_LIB="$ROOT/pdfium/lib"
fi
if [[ -z "${FILECONV_TESSDATA:-}" && -d "$ROOT/tessdata_best" ]]; then
  export FILECONV_TESSDATA="$ROOT/tessdata_best"
fi
if [[ -z "${FILECONV_WHISPER_MODEL:-}" && -f "$ROOT/models/ggml-tiny.bin" ]]; then
  export FILECONV_WHISPER_MODEL="$ROOT/models/ggml-tiny.bin"
fi
if [[ -z "${FILECONV_BIN:-}" && -x "$ROOT/target/release/fileconv" ]]; then
  export FILECONV_BIN="$ROOT/target/release/fileconv"
fi

exec python3 "$ROOT/bench/markhand_web/scripts/run_ingest_capacity.py" "$@"
