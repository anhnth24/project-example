# Kiến trúc hệ thống

> Luồng dữ liệu, định tuyến, và các ranh giới hệ thống. Số liệu đo thực trích [`../bench/`](../bench/).

## Tổng quan 3 lớp

```
┌─────────────────────────────────────────────────────────────────────┐
│  GIAO DIỆN                                                          │
│  ┌──────────────┐   ┌─────────────────────┐   ┌──────────────────┐  │
│  │ CLI (fileconv)│   │ Desktop "Markhand"  │   │ MCP (fileconv-mcp)│  │
│  │ one/speed/   │   │ Tauri 2 + React 19  │   │ stdio / rmcp     │  │
│  │ accuracy/audio│   │ 18 IPC commands     │   │ 8 tools          │  │
│  └──────┬───────┘   └─────────┬───────────┘   └────────┬─────────┘  │
└─────────┼─────────────────────┼────────────────────────┼────────────┘
          │                     │                        │
          └─────────────────────┼────────────────────────┘
                                ▼
┌─────────────────────────────────────────────────────────────────────┐
│  LÕI: fileconv-core  (Converter::convert_path)                      │
│  định tuyến theo FormatKind (extension) → conv::* / image_ocr / audio│
│  → NFC + cắt → ConversionResult                                      │
└─────────────────────────────────────────────────────────────────────┘
                                ▼
┌─────────────────────────────────────────────────────────────────────┐
│  PHỤ THUỘC NATIVE (đắt, có cache)                                   │
│  PDFium (thread_local) · Tesseract (per-call) · Whisper (process cache) │
└─────────────────────────────────────────────────────────────────────┘
```

## Lõi `fileconv-core`

### Định tuyến (`crates/core/src/lib.rs`)

```rust
pub enum FormatKind { Pdf, Docx, Pptx, Xlsx, Csv, Html, Image, Audio, Unknown }
// FormatKind::from_path(&Path) -> Self   // match extension lowercase, KHÔNG sniff magic-byte

pub fn convert_path(&self, path: &Path) -> Result<ConversionResult, ConvertError>
//   1. FormatKind::from_path(path)
//   2. match → conv::{pdf,docx,pptx,xlsx,csv_conv,html}::to_markdown
//                | image_ocr::ocr_image | self.engine()?.transcribe_file
//   3. NFC chuẩn hoá (is_nfc_quick guard) + optional max_chars cắt
//   4. ConversionResult { markdown, title: None, format }
```

`Unknown` → `ConvertError::Unsupported`. Lỗi `ConvertError` (thiserror): `BadPath`, `Unsupported(&str)`, `Failed(String)`.

### Chuỗi fallback PDF (3 tier)

```
convert pdf
   │
   ▼
 pdf-inspector (CHÍNH — có cấu trúc)
   • markdown theo trang: heading theo cỡ chữ, bảng, sắp lại đa cột
   • cờ needs_ocr từng trang (bắt cả text-layer rác / font GID)
   • catch_unwind  ─────────────┐  panic? fallback
   ▼                            ▼
 trang text  → dùng luôn    pdfium-render
   • đếm ký tự (PAGE_TEXT_MIN_CHARS=10)
   • catch_unwind ──┐ panic/thiếu? fallback
   ▼               ▼
                  pdf-extract =0.8.2  (chỉ khi thiếu lib PDFium)
 trang needs_ocr → render PDFium 300 DPI → Tesseract OCR
 pdf_ocr_images (mặc định tắt) → OCR thêm ảnh nhúng ≥200×200px
```

**Thread-safety lock**: libpdfium KHÔNG thread-safe. Mỗi region dùng PDFium được 
serialized qua `PDFIUM_CALL: Mutex<()>` trong `pdf.rs`, gồm cả render+OCR các trang quét. 
Trade-off: concurrent scanned-PDF conversions queue tại lock (move Tesseract ngoài nếu cần throughput cao).

Đánh đổi (đo thực): corpus cũ pdf-inspector **~18ms/trang** (có cấu trúc + đa cột)
vs PDFium **~5.67ms/trang** (chỉ text). Đường range song song mới đo **~7.8ms/trang**
trên PDF CASAN 45 trang/8 vCPU; chọn riêng một trang ~55–59ms thay vì ~400–440ms.
Tauri dev tối ưu tạo sidecar cùng file trong ~0.40s (trước đó 17.54s do core opt-level 0
và không tìm thấy PDFium khi working directory là `app/`).
Chi tiết: [`../bench/REPORT_CASAN_PDF.md`](../bench/REPORT_CASAN_PDF.md).

### Mỗi converter (tóm tắt)

| Format | Crate gốc | Điểm chính |
|---|---|---|
| pdf | pdf-inspector → pdfium-render → pdf-extract | 3-tier như trên |
| docx | docx-rust + OOXML pass | heading/run; gridSpan/vMerge → sanitized HTML table |
| xlsx | calamine 0.35 | mọi sheet; xls/xlsb/ods; merge/multiline → rowspan/colspan |
| pptx | zip + quick-xml | text Markdown + structured preview text/image/shape |
| html | htmd 0.5 | skip script/style/noscript; thay html2md (cũ phình output) |
| csv/text | csv + legacy maps | UTF-8/TCVN3/VNI/VPS; delimiter sniff; plain `.txt/.log` |
| image | Tesseract/Paddle | preprocess, split scan columns, PSM retry; Paddle opt-in/fallback |
| audio | whisper-rs 0.16 + symphonia 0.5 | 16k mono; tự tìm PhoWhisper; lọc segment no-speech/marker nhạc |

### Post-processing
Mọi output: NFC (bắt buộc) → optional cắt tại `max_chars` kèm `<!-- (đã cắt...) -->`.

### ConverterOptions
`ocr_langs`, `ocr_engine` (`tesseract|auto|paddle`), `whisper_model`, `audio_lang` (`"vi"`),
`audio_threads` (4), `audio_no_speech_threshold` (0.6), `pdf_ocr` (true),
`pdf_ocr_images` (false), `pdf_pages: Option<Vec<u32>>` (1-indexed),
`xlsx_sheet: Option<String>`, `max_chars: Option<usize>`.

### Module công khai
`audio` (AudioEngine, Transcript, decode_to_pcm16k_mono) · `image_ocr` (ocr_image, ocr_dynimage, tesseract_available) ·
`chunk` · `probe` · `pptx_preview` · `tables` · `viet_legacy`
(TCVN3/VNI/VPS detect/decode) ·
`llm`/`llm_cli` (HTTP chat/vision, neural embeddings, Cursor/Codex subscription transport).
`conv::*` **private** — chỉ qua `convert_path`.

## CLI `fileconv` (`crates/cli`)

| Subcommand | Args | Mục đích |
|---|---|---|
| `one` | `<file> [--ocr-images --lang vie+eng --pages 1,2,3 --sheet NAME --max-chars N]` | convert 1 file → stdout |
| `speed` | `<dir> [report.md]` | ms/file, ms/page, KB/s theo format (`count_pages` gọi pdfinfo/python3) |
| `accuracy` | `<manifest.tsv> [report.md]` | CER/WER (Levenshtein, `normalize()` bỏ markdown) theo nhãn |
| `audio` | `<models.csv> <manifest.tsv> [report.md]` | WER/RTF/load mỗi model GGML |
| `pptx-preview` | `<file.pptx>` | JSON preview meta/slides/shapes cho QA |

Panic hook in `file:line`. Manifest: mỗi dòng `<file>\t<ground_truth.txt>\t<nhãn>`, `#` = comment.

## MCP server `fileconv-mcp` (`crates/mcp`)

Transport **stdio** qua `rmcp 0.16`, tokio multi-thread, build với feature `llm`. Mọi tool body chạy trong
`spawn_blocking` (convert là blocking I/O). Lỗi dạng `Result<String,String>`. Đăng ký:
`claude mcp add fileconv -- .../fileconv-mcp`.

| Tool | Loại | Mục đích |
|---|---|---|
| `detect_format` | deterministic | `probe()` → format/bytes/pages/sheets JSON, không convert |
| `convert_to_markdown` | deterministic | convert đầy đủ (đọc `FILECONV_WHISPER_MODEL`) |
| `extract_tables_json` | deterministic | xlsx/csv → JSON rows (`tables::tables_json`) |
| `convert_chunks` | deterministic | convert + `chunk::chunk_markdown` → JSON `[{index,heading,text,chars}]` |
| `summarize` | LLM (`FILECONV_LLM_*`) | tóm tắt (cap 40 000 ký tự) |
| `extract_json` | LLM | trích JSON theo hướng dẫn ngôn ngữ tự nhiên |
| `translate` | LLM | dịch sang ngôn ngữ đích |
| `ocr_hard` | LLM (vision) | OCR ảnh khó (đa cột, IN HOA, viết tay, con dấu) — chất lượng hơn Tesseract |

`ocr_hard` (`llm.rs::vision_ocr`): base64 ảnh → POST vision endpoint của provider (OpenAI/Anthropic/Gemini),
system prompt yêu cầu phiên âm Markdown tiếng Việt trung thực, timeout 180s.

Desktop cấu hình provider trực tiếp trong Settings, ưu tiên local Ollama/LM Studio/
llama.cpp/vLLM; cloud có OpenAI, Anthropic, Gemini, OpenRouter, Groq, Mistral,
Together và custom OpenAI-compatible. Cursor/Codex subscription chạy qua CLI
chính thức ở ask/read-only sandbox; Claude consumer OAuth bị loại theo policy
Anthropic. API key chỉ giữ trong memory. Chi tiết:
[`llm-providers.md`](llm-providers.md).

## Desktop "Markhand" (`app/`)

**Stack**: Tauri 2 + React 19.2 + Vite 6 + TypeScript strict + Zustand 5 + UI primitives nội bộ theo
LumiBase + lucide-react. Editor: CodeMirror, react-markdown+GFM+raw HTML sanitizer,
pdfjs-dist 6.1, docx-preview,
@e965/xlsx. Font Inter Variable được bundle offline. Rust phụ thuộc `fileconv-core`
(path `../../crates/core`). Plugins: `tauri-plugin-updater`, `tauri-plugin-process`.

**Identity** (`tauri.conf.json`): productName/binary `Markhand`/`markhand`,
identifier `com.anhnth24.markhand`, v0.1.0 (đây là source-of-truth cho version),
cửa sổ 1440×900 (min 900×600). Permission tối thiểu: `core:default`, `dialog:default`, `opener:default`,
`updater:default`, `process:default` — **không** fs scope (mọi FS qua custom command). 
Rust crate `fileconv-desktop`. Bundle có icon đa nền tảng, metadata deb/AppImage/MSI/DMG 
và CI release matrix; `.deb` Linux đã build thực tế.

**Auto-update** (`tauri.conf.json`): `plugins.updater` với endpoint 
`https://github.com/anhnth24/project-example/releases/latest/download/latest.json` 
và minisign public key để xác minh signatures. Artifact ký (updater artifacts) tự động 
sinh qua `createUpdaterArtifacts: true` từ CI. Desktop khởi động background check update 
(non-blocking, bỏ qua lỗi mạng) và hiển thị thông báo nếu có version mới; người dùng cài từ Settings.

### Luồng UI → Rust

```
  drop / pick file (dialog)             whole-window onDragDropEvent (App.tsx)
        │                                              │
        └───────────────┬──────────────────────────────┘
                        ▼
        api.importFile(activeFolder, sourceAbs)  → invoke("import_file")
                        ▼  [Rust]
  import_file_only: validate FormatKind → copy nguyên tử vào DATA root → trả Node raw
                  Zustand queue gọi reconvert tuần tự trong background
                  spawn_blocking(convert_and_write_md(opts, dest))
                        │   Settings(mutex) → ConverterOptions
                        │   Converter::convert_path(source)   ← fileconv-core
                        ▼
                  ghi <source>.md kề source  → trả Node{mdRelPath}
                        ▼  [Frontend]
   refreshTree() → DocView đọc markdown (read_text_file)
                  → Soạn (CodeMirror) / Save (write_text_file) / Reconvert
```

### Cầu nối Tauri (`app/src-tauri/src/lib.rs`)
- **AppState**: config/DATA/settings + `WatchService` notify thread.
- **Path safety**: `resolve_within` chặn `..`, tuyệt đối, root-relative.
- **56 command**: `supported_extensions`, `get/set_data_root` (persist `config.json`; mặc định `app_data_dir()/DATA`),
  `read_tree` (ghép cặp `report.pdf`↔`report.pdf.md` 1-1, ẩn phía `.md`, đánh dấu `standaloneMd`),
  `create_folder`, `create_markdown`, `rename_node` (rename cả `.md` ghép cặp), `delete_node` (từ chối xóa DATA root),
  `import_file_only` (copy, chưa convert), `import_file` (compat: copy + convert), `reconvert`,
  `read/write_text_file`, `read_text_preview` (head + truncated),
  `file_size`, `resolve_path`, `read_bytes` (ArrayBuffer — webview `fetch(asset://)` trả 403 nên phải dùng command này),
  `get/set_settings`; cộng nhóm Intelligence: handoff BRD/PRD, quality, cited search/Q&A,
  PII/redaction, schema/tables/CSV, versions/diff/merge, watch rules, hard OCR và ZIP pack.
  RAG mới gồm `rebuild_knowledge_index`, `knowledge_index_stats`, `hybrid_search`,
  `hybrid_ask`: SQLite FTS5 + local hash 256D hoặc neural embeddings
  OpenAI-compatible/Gemini, model signature/dimension guard, incremental content
  hash, persistent HNSW (>1.000 chunks), exact fallback, RRF rerank, anchors và
  FTS/LLM fallback có validation citation.
  Bốn command cấu hình mới: subscription status/login và embedding presets/test.
  PPTX meta/slide preview và live-watch status là ba command còn lại.
  Nhóm Project: list/create/adopt/remove project và import đệ quy folder local.
- `convert_and_write_md` map `Settings`→`ConverterOptions`→`Converter::convert_path`→ghi `.md`.

### State (Zustand, `app/src/state/store.ts`)
Store duy nhất, **không persist nội dung** (Rust/filesystem là nguồn sự thật). Ngoài DATA tree/settings, store giữ
`openTabs`, `activeTab`, draft session theo `relPath`, view Home/Library/Document/Intelligence và hàng đợi convert tuần tự.
Baseline của chế độ đối chiếu được cache local để không thay đổi khi người dùng lưu draft. Types IPC vẫn mirror
struct serde camelCase của Rust.

### <a id="theme"></a>Theme
App dùng dark cosmic theme theo LumiBase: nền gradient, glass hairline, Inter và accent lục `#2EC47C`.
Token production nằm trong `styles.css`; không chạy runtime HTML/JS của prototype. Bề mặt file nguồn
(PDF/DOCX/Excel) vẫn sáng để giữ đúng hình thức tài liệu và độ tương phản.

## Tích hợp MCP vào Claude Code

```bash
claude mcp add fileconv -- /đường/dẫn/fileconv-mcp
# tool LLM cần env: FILECONV_LLM_API_KEY, FILECONV_LLM_BASE_URL, FILECONV_LLM_MODEL ...
```

## Tham chiếu chéo
- Map file: [`codebase-summary.md`](codebase-summary.md)
- Quy ước & cạm bẫy: [`code-standards.md`](code-standards.md)
- Số liệu đo: [`../bench/REPORT.md`](../bench/REPORT.md)
