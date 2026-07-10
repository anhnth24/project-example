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
│  PDFium (thread_local) · Tesseract (per-call) · Whisper (OnceLock)  │
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

Đánh đổi (đo thực): corpus cũ pdf-inspector **~18ms/trang** (có cấu trúc + đa cột)
vs PDFium **~5.67ms/trang** (chỉ text). Đường range song song mới đo **~11.4ms/trang**
trên PDF CASAN 45 trang/8 vCPU; chọn riêng một trang ~55–61ms thay vì ~400–440ms.
Chi tiết: [`../bench/REPORT_CASAN_PDF.md`](../bench/REPORT_CASAN_PDF.md).

### Mỗi converter (tóm tắt)

| Format | Crate gốc | Điểm chính |
|---|---|---|
| pdf | pdf-inspector → pdfium-render → pdf-extract | 3-tier như trên |
| docx | docx-rust 0.1.11 | style→heading; gom run theo (bold,italic) trước khi bao emphasis (tránh dính chữ); `<w:br>/<w:tab>` |
| xlsx | calamine 0.35 (`open_workbook_auto`) | đọc MỌI sheet; hỗ trợ xls/xlsb/ods; `rows_to_md_table` chung |
| pptx | zip 2.2 + quick-xml 0.37 | enumerate `ppt/slides/slideN.xml`, **sort theo N** (sửa lỗi zip-order) |
| html | htmd 0.5 | skip script/style/noscript; thay html2md (cũ phình output) |
| csv | csv 1.3 | strip BOM; TCVN3 fallback (`viet_legacy`); sniff delimiter `, ; \t \|` |
| image | Tesseract CLI | tiền xử lý: grayscale → upscale×2 nếu nhỏ → unsharpen → normalize |
| audio | whisper-rs 0.16 + symphonia 0.5 | decode + resample 16k mono; lang "vi"; ưu tiên PhoWhisper |

### Post-processing
Mọi output: NFC (bắt buộc) → optional cắt tại `max_chars` kèm `<!-- (đã cắt...) -->`.

### ConverterOptions
`ocr_langs` (mặc định `"vie+eng"`), `whisper_model: Option<PathBuf>`, `audio_lang` (`"vi"`),
`audio_threads` (4), `pdf_ocr` (true), `pdf_ocr_images` (false), `pdf_pages: Option<Vec<u32>>` (1-indexed),
`xlsx_sheet: Option<String>`, `max_chars: Option<usize>`.

### Module công khai
`audio` (AudioEngine, Transcript, decode_to_pcm16k_mono) · `image_ocr` (ocr_image, ocr_dynimage, tesseract_available) ·
`chunk` (chunk_markdown, chunks_json, Chunk) · `probe` (FileInfo) · `tables` (tables_json) · `viet_legacy` (decode_text, looks_like_tcvn3, decode_tcvn3) ·
`llm` (chỉ feature `llm`: LlmConfig::from_env, chat, summarize, extract_json, vision_ocr).
`conv::*` **private** — chỉ qua `convert_path`.

## CLI `fileconv` (`crates/cli`)

| Subcommand | Args | Mục đích |
|---|---|---|
| `one` | `<file> [--ocr-images --lang vie+eng --pages 1,2,3 --sheet NAME --max-chars N]` | convert 1 file → stdout |
| `speed` | `<dir> [report.md]` | ms/file, ms/page, KB/s theo format (`count_pages` gọi pdfinfo/python3) |
| `accuracy` | `<manifest.tsv> [report.md]` | CER/WER (Levenshtein, `normalize()` bỏ markdown) theo nhãn |
| `audio` | `<models.csv> <manifest.tsv> [report.md]` | WER/RTF/load mỗi model GGML |

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

## Desktop "Markhand" (`app/`)

**Stack**: Tauri 2 + React 19.2 + Vite 6 + TypeScript strict + Zustand 5 + UI primitives nội bộ theo
LumiBase + lucide-react. Editor: CodeMirror, react-markdown+remark-gfm, pdfjs-dist 6.1, docx-preview,
@e965/xlsx. Font Inter Variable được bundle offline. Rust phụ thuộc `fileconv-core`
(path `../../crates/core`).

**Identity** (`tauri.conf.json`): productName `Markhand`, identifier `com.anhnth24.fileconv-docs`, v0.1.0,
cửa sổ 1440×900 (min 900×600). Permission tối thiểu: `core:default`, `dialog:default`, `opener:default`
— **không** fs scope (mọi FS qua custom command). Rust crate `fileconv-desktop`.

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
- **AppState**: `{ config_dir, data_root: Mutex<PathBuf>, settings: Mutex<Settings> }`.
- **Path safety**: `resolve_within` chặn `..`, tuyệt đối, root-relative.
- **19 command**: `supported_extensions`, `get/set_data_root` (persist `config.json`; mặc định `app_data_dir()/DATA`),
  `read_tree` (ghép cặp `report.pdf`↔`report.pdf.md` 1-1, ẩn phía `.md`, đánh dấu `standaloneMd`),
  `create_folder`, `create_markdown`, `rename_node` (rename cả `.md` ghép cặp), `delete_node` (từ chối xóa DATA root),
  `import_file_only` (copy, chưa convert), `import_file` (compat: copy + convert), `reconvert`,
  `read/write_text_file`, `read_text_preview` (head + truncated),
  `file_size`, `resolve_path`, `read_bytes` (ArrayBuffer — webview `fetch(asset://)` trả 403 nên phải dùng command này),
  `get/set_settings`.
- `convert_and_write_md` map `Settings`→`ConverterOptions`→`Converter::convert_path`→ghi `.md`.

### State (Zustand, `app/src/state/store.ts`)
Store duy nhất, **không persist nội dung** (Rust/filesystem là nguồn sự thật). Ngoài DATA tree/settings, store giữ
`openTabs`, `activeTab`, draft session theo `relPath`, view Home/Library/Document và hàng đợi convert tuần tự.
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
