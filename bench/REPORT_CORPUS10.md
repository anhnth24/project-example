# Markhand CORPUS10 — internet compatibility and quality report

Date: 2026-07-11. Binary: optimized release build. Corpus provenance and SHA-256
are pinned in [`CORPUS10_SOURCES.lock.json`](CORPUS10_SOURCES.lock.json).

## Scope

- **90 public internet files**.
- Exactly 10 samples for each family: PDF, DOCX, PPTX, spreadsheet
  (XLSX/XLS/XLSB/ODS), CSV, HTML, image OCR, audio and plain text.
- Every download passed magic-byte/container validation and SHA-256 verification.
- The batch runner calls the same `fileconv_core::Converter` used by Markhand
  desktop.
- Separate desktop smoke tests opened a real PPTX and merged XLSX in the running
  Tauri application.

## Result

- Conversion completed: **90/90 (100%)**.
- Heuristic content errors: **0**.
- Warnings: **2**, both intentionally sparse upstream fixtures
  (`doc-default.docx`, `issue127.xls`).
- Unicode replacement-character failures: **0**.
- HTML script/style leakage: **0**.
- CSV table generation: **10/10**.
- PDF output non-empty: **10/10**.
- Image OCR output non-empty: **10/10**.
- Audio music/codec fixtures suppressed as non-speech: **9/10**; one WAV retained
  a four-character fragment and is reported as informational residual risk.

Raw details:

- [`REPORT_CORPUS10_SPEED.md`](REPORT_CORPUS10_SPEED.md)
- [`REPORT_CORPUS10_QUALITY.md`](REPORT_CORPUS10_QUALITY.md)

## Release performance

| Family | Files | Successful | Mean ms/file | Notes |
|---|---:|---:|---:|---|
| PDF | 10 | 10 | 83.71 | 139 pages, **6.02 ms/page** |
| DOCX | 10 | 10 | 0.50 | Paragraphs, links, headers and tables |
| PPTX | 10 | 10 | 0.14 | Markdown text extraction |
| Spreadsheet | 10 | 10 | 0.09 | XLSX/XLS/XLSB/ODS |
| CSV | 10 | 10 | 0.46 | Up to 10,000 rows |
| HTML | 10 | 10 | 9.59 | Includes large English/Vietnamese Wikipedia pages |
| Image OCR | 10 | 10 | 603.79 | Tesseract CPU, includes real two-column sample |
| Audio | 10 | 10 | 1,037.64 | Whisper tiny CPU, model loaded once |
| Text | 10 | 10 | 0.93 | Gutenberg files up to 1.26M characters |

## Feature-specific checks

### PPTX preview

- Structured preview command parsed all **10/10** presentations.
- Running Markhand rendered a real two-slide PPTX with embedded PNG images,
  previous/next controls and side-by-side Markdown.
- Text, images, grouped basic shapes and chart placeholders are represented.
- Complex SmartArt/chart internals remain placeholders; the UI states this and
  retains **Open externally**.

### Merged tables

- `merged_range.xlsx` and `.xls` emitted HTML tables with combined
  `rowspan`/`colspan`.
- Running Markhand rendered those tables rather than exposing raw tags.
- Raw HTML passes `rehype-sanitize`; untrusted scripts and attributes are not
  executed.

### OCR

- Tesseract two-column OCR preserved left-column-then-right-column reading order.
- Optional PaddleOCR 3.7 + PaddlePaddle 3.3 was installed in an isolated test
  environment and invoked through the release converter.
- On `tesseract-2col.png`, Paddle returned 1,305 characters with clean column
  ordering; Tesseract returned 1,323 with several short noise fragments.
- Paddle remains opt-in because its Python/models footprint is large; any
  runtime/import failure falls back to Tesseract.
- A generic local OpenAI-compatible VLM preset supports user-selected on-device
  vision models without committing a model identifier.

### Audio hallucination

The first run produced bracketed pseudo-speech on nine music/codec samples.
After adding generic bracketed-marker suppression, the final run returned empty
transcripts for nine samples and only one four-character fragment. Real speech
accuracy remains covered by the dedicated Vietnamese audio manifest; these ten
files have no transcript ground truth.

### Legacy Vietnamese encodings

VNI-Windows and VPS maps were generated from VietUnicode's 134-character
reference table. Explicit decoders, conservative detectors, cross-routing tests
and `.txt` conversion are implemented alongside TCVN3.

## Desktop verification

The Tauri app was run under Xvfb at 1440×900:

1. Created a project.
2. Loaded real corpus PPTX/XLSX documents.
3. Opened PPTX source preview and changed slides.
4. Verified embedded images render.
5. Opened merged XLSX in split mode.
6. Verified source sheet merges and sanitized Markdown HTML table render.

## Remaining external blockers

- A signed Windows installer requires an Authenticode PFX or Azure Trusted
  Signing credentials.
- A signed/notarized macOS DMG requires Apple Developer ID certificate and
  notarization credentials on a macOS runner.
- The workflows now produce unsigned artifacts when credentials are absent and
  switch to signed verification when repository variables/secrets are enabled.
- No signing secret or certificate is committed to the repository.
