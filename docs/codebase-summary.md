# Tóm tắt codebase & Bản đồ điều hướng

> Đâu để tìm cái gì, khi cần sửa thì đụng file nào.

## Bức tranh tổng thể

```text
crates/core/       shared conversion engine and detailed diagnostics
crates/cli/        conversion CLI, handoff and benchmark commands
crates/mcp/        stdio MCP surface, deterministic and optional LLM tools
crates/knowledge/  framework-free retrieval/grounding contracts plus opt-in desktop adapters
crates/server/     Markhand Web API, auth, storage adapters and workers
app/               Markhand Tauri desktop
web/               browser-only React/Vite SPA over HTTP/SSE
deploy/            local and POC infrastructure, scripts and observability
bench/             reproducible converter/Web evaluation assets and reports
plans/             authoritative Markhand Web issue catalog and evidence
vendor/            reference-only markitdown-rs, excluded from the workspace
```

**CLI, desktop và MCP dùng chung `fileconv-core`**: cả ba path-dep thẳng vào
`crates/core` (`Converter`/`FormatKind`) — không re-implement logic convert ở đâu
khác. **Markhand Web đi theo boundary riêng**: `web/` (browser SPA) chỉ nói
HTTP/SSE với `fileconv-server` (`crates/server`) — server sở hữu auth/services/
repositories/storage adapters và các worker; phần retrieval/grounding (search,
Q&A, chunk/embedding contract) đi qua `fileconv-knowledge` (`crates/knowledge`)
dùng chung giữa server và các adapter desktop opt-in (SQLite/HNSW), không phải
`fileconv-core`.

## `crates/core/` — fileconv-core (engine)

| File | Trách nhiệm |
|---|---|
| `src/lib.rs` | `Converter`, `FormatKind` (Pdf, Docx, Pptx, Xlsx, Csv, Html, **Text**, Image, Audio, Unknown), `ConverterOptions`; `convert_path_detailed()` là API chính (NFC + cắt output + soft warnings + derive title); `convert_path()` là lớp compatibility mỏng gọi `convert_path_detailed()` rồi bỏ warnings |
| `src/conv/mod.rs` | khai báo module convert theo format |
| `src/conv/pdf/` | pipeline PDF **3-tier**, chia module: `mod.rs` (điều phối), `inspector.rs` (pdf-inspector — cấu trúc + `needs_ocr`), `pdfium.rs` (bind libpdfium, cache `thread_local`, khóa `PDFIUM_CALL`), `native_text.rs`, `ocr.rs` (render 300DPI + Tesseract), `fallback.rs` (pdf-extract), `recovery.rs`, `postprocess.rs` |
| `src/conv/docx.rs` | docx-rust: heading theo style, gom run theo (bold,italic), xử lý `<w:br>/<w:tab>` |
| `src/conv/xlsx.rs` | calamine: đọc MỌI sheet (xls/xlsb/ods) |
| `src/conv/pptx.rs` | zip + quick-xml: slide sort theo số thứ tự |
| `src/conv/html.rs` | htmd (skip script/style/noscript) |
| `src/conv/csv_conv.rs` | csv: strip BOM, TCVN3 fallback, sniff delimiter, chứa `rows_to_md_table` chung |
| `src/conv/text.rs` | txt/log/md/markdown: strip BOM UTF-8 + decode qua `viet_legacy` |
| `src/proc.rs` | `background_command()`: CREATE_NO_WINDOW trên Windows (tránh flash console) |
| `src/image_ocr.rs` | Tesseract CLI + tiền xử lý ảnh (grayscale/upscale/unsharpen/normalize); dùng `proc::background_command` |
| `src/audio.rs` | (feature `audio`) AudioEngine — cache Whisper **process-wide** (bounded LRU), decode symphonia + resample 16k, lang "vi" |
| `src/chunk.rs` | tách chunk RAG theo heading-path |
| `src/viet_legacy.rs` | decode TCVN3/VNI/VPS; opt-in `Tcvn3CaseHint` (TCVN3/ABC all-capital H-font) — TXT/CSV không suy hoa |
| `src/diagnostics.rs` | `ConversionReport`/`ConversionWarning`/`ConversionOutcome`/`DetailedConvertError` — soft warnings và outcome (FullSuccess/PartialSuccess) trả về từ `convert_path_detailed`, tách khỏi lỗi cứng `ConvertError` |
| `src/embedding_runtime.rs` | helper runtime-path embedding luôn bật (không gate `llm`, ADR 0006) — dùng chung giữa `llm.rs` và `fileconv-knowledge` để tránh cycle phụ thuộc |
| `src/intelligence.rs` | document intelligence trên Markdown sidecar: handoff pack, cited search/Q&A, quality, PII, table/schema, version — baseline tất định, LLM chỉ tăng cường (feature `llm`) |
| `src/llm.rs` | (feature `llm`) chat/summarize/extract_json/vision_ocr qua env `FILECONV_LLM_*` |
| `src/llm_cli.rs` | (feature `llm`) transport CLI subscription Cursor/Codex chính thức (không phải Claude consumer OAuth) |
| `src/probe.rs` | `probe()` → FileInfo{format,bytes,pages,sheets} |
| `src/tables.rs` | `tables_json` (xlsx/csv → JSON rows) — LƯU Ý: không decode TCVN3 như csv_conv.rs |
| `src/pptx_preview.rs` | metadata + preview slide/shape cho desktop SVG, cũng là nguồn PPTX metadata cho CLI `pptx-preview` |

## `crates/cli/` — binary `fileconv`

Tám subcommand đăng ký ở `registered_commands()` (`crates/cli/src/main.rs`):

| Lệnh | Mục đích |
|---|---|
| `one` | convert 1 file → stdout (markdown thuần, legacy) |
| `one-detailed` | convert + JSON `{markdown,title,format,outcome,warnings}` hoặc lỗi `{message,kind}` |
| `speed` | bench tốc độ ms/file, ms/page, KB/s theo format trên một thư mục |
| `accuracy` | CER/WER (Levenshtein, `normalize()` bỏ markdown) theo manifest |
| `audio` | (feature `audio`) WER/RTF/load mỗi model GGML |
| `handoff` | đóng gói handoff (BRD/PRD) từ nhiều file nguồn → ZIP |
| `pptx-preview` | JSON preview meta/slides/shapes; metadata PPTX lấy qua `fileconv_core::pptx_preview`/`probe`, không còn shell `python3` |
| `info` | danh sách định dạng hỗ trợ + trạng thái PDFium/tessdata/model whisper |

## `crates/mcp/` — binary `fileconv-mcp`

Chín MCP tool (stdio/rmcp) — năm deterministic (không cần API key) và bốn LLM
(cần `FILECONV_LLM_*`):

| Tool | Loại | Mục đích |
|---|---|---|
| `detect_format` | deterministic | `probe()` → format/bytes/pages/sheets, không convert |
| `convert_to_markdown` | deterministic | convert → markdown thuần (legacy) |
| `convert_to_markdown_detailed` | deterministic | convert → JSON có `outcome`/`warnings` (partial success) |
| `extract_tables_json` | deterministic | xlsx/csv → JSON rows |
| `convert_chunks` | deterministic | convert + chia chunk RAG theo heading |
| `summarize` | LLM | tóm tắt tài liệu |
| `extract_json` | LLM | trích JSON theo hướng dẫn ngôn ngữ tự nhiên |
| `translate` | LLM | dịch sang ngôn ngữ đích |
| `ocr_hard` | LLM (vision) | OCR ảnh khó (đa cột, IN HOA, viết tay, con dấu) |

## `crates/knowledge/` — fileconv-knowledge (contracts)

Framework-free: `types`, `query`, `rank`, `embedding`, `citation`, `ask`,
`identity`, `error` là contract thuần (không phụ thuộc storage/transport cụ
thể). Desktop-only adapter nằm sau feature riêng:

| File | Trách nhiệm |
|---|---|
| `src/desktop/sqlite.rs` | (feature `desktop-sqlite`) SQLite FTS5 — index/tra cứu chunk local |
| `src/desktop/hnsw.rs` | (feature `desktop-hnsw`) persistent HNSW ANN cache cho vector local |
| `src/desktop/service.rs` | ghép SQLite + HNSW thành service hybrid-search/ask cho desktop |

## `crates/server/` — fileconv-server (Markhand Web API)

| Vùng | File chính | Trách nhiệm |
|---|---|---|
| routes | `src/routes/*.rs` | HTTP handler theo tài nguyên: `health`, `auth`, `uploads`, `collections`, `documents`, `jobs`, `members`, `orgs`, `projects`, `audit`, `search`, `ask`, `chat_sessions`, `events`, `graph` — ghép ở `src/http.rs::router` |
| services | `src/services/*.rs` | logic domain: upload saga/sniff/limits, conversion, indexing/chunking, embedding, citation, access/ACL, quota, deletion, graph |
| repositories | `src/db/*.rs` | truy vấn PostgreSQL theo bảng/nghiệp vụ (jobs, documents, collections, chunks, orgs, members, audit, acl_sql, ...) |
| storage adapters | `src/storage/*.rs` | `minio.rs` (object store), `qdrant.rs` (vector index), `keys.rs`/`url_safety.rs` |
| workers | `src/workers/*.rs` | `convert.rs`, `index.rs`, `embedding.rs`, `delete.rs`, `reconcile.rs`, `sandbox.rs`, `fairness.rs`, `limits.rs` — chạy qua `src/bin/worker.rs`, tách khỏi tiến trình API |

## `app/` — desktop "Markhand" (Tauri 2)

| Vùng | File chính | Trách nhiệm |
|---|---|---|
| entry | `src/main.tsx`, `src/App.tsx` | root + drag-drop toàn cửa sổ + toast lỗi |
| Home | `components/HomeView.tsx`, `IconRail.tsx` | điều hướng vùng chính, tổng quan project |
| Library | `components/LibraryView.tsx`, `Sidebar.tsx`, `Tree.tsx` | cây file, toolbar upload/tạo, đổi DATA root |
| Document | `components/DocView.tsx`, `DocumentTabs.tsx`, `MarkdownEditor.tsx`, `SourcePreview.tsx`, `CompareView.tsx` | workspace tab split/md/source, Soạn/Xem trước, đối chiếu version |
| Intelligence | `components/IntelligenceView.tsx` | handoff, quality, cited search/Q&A, PII, schema/table, versions |
| conversion queue | `components/ConvertQueue.tsx` | hàng đợi reconvert tuần tự chạy background |
| project | `src-tauri/src/projects.rs` | list/create/adopt/remove project, import đệ quy folder local |
| Settings | `components/Settings.tsx` | modal cài OCR lang, PDF OCR, audio lang/threads, model whisper, LLM/embedding |
| state | `state/store.ts` | Zustand store duy nhất (không persist — Rust là nguồn sự thật) |
| lib | `lib/ipc.ts`, `lib/types.ts` | wrap `invoke` + type `FsNode`/`Settings` mirror Rust serde |
| Tauri bridge | `src-tauri/src/lib.rs` | `AppState`, `convert_and_write_md`/`convert_and_write_md_detailed`; đăng ký toàn bộ command qua `tauri::generate_handler!` — registry là macro đó, không phải một con số cố định |
| | `src-tauri/src/main.rs` | shim gọi `run()` |
| config | `src-tauri/tauri.conf.json`, `capabilities/default.json` | identity Markhand, permission tối thiểu |

`generate_handler!` gom các nhóm lệnh: cây/file DATA (`read_tree`, `create_folder`,
`create_markdown`, `rename_node`, `delete_node`, `read/write_text_file`, ...),
import/convert (`import_file_only`, `import_file`, `reconvert` và bản song song
`reconvert_detailed` — cùng side effect, thêm `outcome`/`warnings`), settings/
watch, preview PPTX, nhóm Intelligence (handoff, quality, cited search/Q&A, PII,
schema/table, version, watch rules), nhóm knowledge RAG (`rebuild_knowledge_index`,
`knowledge_index_stats`, `hybrid_search`, `hybrid_ask`) và nhóm project.

## `web/` — browser SPA (`web/src/pages/`)

| Trang | File | Vùng |
|---|---|---|
| Login | `LoginPage.tsx` | đăng nhập/khởi tạo session |
| Library | `LibraryPage.tsx` | library/upload tài liệu qua HTTP |
| Q&A | `QaPage.tsx` | hỏi-đáp/tìm kiếm có trích dẫn qua SSE |
| Graph | `GraphPage.tsx` | xem quan hệ tài liệu/collection |
| Admin | `AdminMembersPage.tsx`, `AdminProjectsPage.tsx`, `AdminUsagePage.tsx` | quản trị member/project/usage |
| Help | `HelpPage.tsx` | hướng dẫn sử dụng |

## `bench/` — đo lường & tái tạo dữ liệu

- **Báo cáo (số liệu đo thực)**: `REPORT.md`, `REPORT_SPEED.md`, `REPORT_ACCURACY.md`,
  `REPORT_AUDIO.md`, `REPORT_PHOWHISPER.md`, `REPORT_EDGE.md`, `REPORT_SAMPLE10*.md`, `REPORT_XL.md`,
  `RESEARCH_COMPETITORS.md`.
- **Shell**: `download_corpus*.sh`, `download_models.sh`, `download_pdfium.sh`,
  `download_tessdata.sh`, `make_sample10.sh`, `make_vn_images.sh`, `make_xl_images.sh`.
- **Python**: `make_vn_corpus.py`, `make_vn_audio.py`, `ocr_experiment.py`, `paddle_test.py`.

> Các thư mục `pdfium/`, `tessdata_best/`, `models/`, `bench/corpus*`, `bench/edge` đều **gitignore**
> — phải chạy script `bench/*.sh` để tái tạo.

## Khi cần sửa — tìm đâu

| Muốn... | Đụng file |
|---|---|
| Thêm / sửa định dạng | `crates/core/src/conv/<fmt>.rs` (hoặc `conv/pdf/` cho PDF) + định tuyến ở `lib.rs` |
| Đổi tiền xử lý OCR | `crates/core/src/image_ocr.rs` |
| Đổi PDFium cache/lock | `crates/core/src/conv/pdf/pdfium.rs` |
| Đổi phụ thuộc native OCR (Tesseract/whisper) | `image_ocr.rs` / `audio.rs` |
| Spawn subprocess (CLI, OCR, LLM) | dùng `crate::proc::background_command` chứ không `Command::new` (tránh console flash Windows) |
| Thêm CLI flag | `crates/cli/src/main.rs` |
| Thêm MCP tool | `crates/mcp/src/main.rs` (+ `crates/core/src/llm.rs` nếu cần LLM) |
| Sửa GUI desktop | `app/src/components/*.tsx` |
| Sửa cầu nối Tauri | `app/src-tauri/src/lib.rs` |
| Sửa Markhand Web API | `crates/server/src/routes/`, `services/`, `db/`, `workers/` |
| Sửa retrieval/grounding contract | `crates/knowledge/src/` |
| Sửa web SPA | `web/src/pages/` |
| Đo lại sau đổi | `bench/` + `fileconv speed`/`accuracy` |

## Tham chiếu chéo
- Quy ước & cạm bẫy khi sửa: [`code-standards.md`](code-standards.md)
- Luồng kiến trúc: [`system-architecture.md`](system-architecture.md)
