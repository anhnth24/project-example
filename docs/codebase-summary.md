# Tóm tắt codebase & Bản đồ điều hướng

> Đâu để tìm cái gì, khi cần sửa thì đụng file nào. Số liệu LOC tính trên file được git track.

## Bức tranh tổng thể

```
project-example/
├── crates/
│   ├── core/    # fileconv-core: LỎI convert (gọi crate gốc), dùng chung bởi CLI + app + MCP
│   ├── cli/     # fileconv: binary CLI + bench harness (đo tốc độ/CER/WER)
│   └── mcp/     # fileconv-mcp: MCP server cho Claude Code (stdio/rmcp)
├── app/         # Markhand: desktop app Tauri 2 + React 19 (GUI)
├── bench/       # script tải corpus + sinh dữ liệu VN + các REPORT*.md (số liệu đo thực)
├── vendor/      # markitdown-rs — CHỈ tham khảo (MIT, đã exclude khỏi workspace)
├── docs/        # tài liệu này
└── CLAUDE.md    # hướng dẫn nhanh cho agent
```

**Một lõi, ba giao diện**: `fileconv-core` là engine duy nhất. CLI, app desktop, và MCP server
đều phụ thuộc/path-dep vào nó — không re-implement logic convert ở đâu khác.

## Quy mô (file được git track)

| Ngôn ngữ | LOC | Files | Nơi chính |
|---|---:|---:|---|
| Rust `.rs` | ~5 669 | 43 | `crates/` + `app/src-tauri/` |
| TypeScript `.tsx` | ~1 140 | 9 | `app/src/components/` |
| TypeScript `.ts` | ~184 | 5 | `app/src/{lib,state}/` |
| Python `.py` | ~2 171 | 8 | `bench/` (sinh corpus + thí nghiệm OCR) |
| CSS | ~1 108 | 1 | `app/src/styles.css` |
| Shell `.sh` | ~391 | 8 | `bench/` (tải corpus/model/pdfium/tessdata) |

## `crates/core/` — fileconv-core (engine)

| File | Trách nhiệm |
|---|---|
| `src/lib.rs` | `Converter`, `convert_path()` (định tuyến), `FormatKind`, `ConverterOptions`, NFC + cắt output |
| `src/conv/mod.rs` | khai báo module convert theo format |
| `src/conv/pdf.rs` | **3-tier**: pdf-inspector (cấu trúc) → pdfium-render → pdf-extract; render trang quét 300DPI + OCR; `PDFIUM_CALL` lock |
| `src/conv/docx.rs` | docx-rust: heading theo style, gom run theo (bold,italic), xử lý `<w:br>/<w:tab>` |
| `src/conv/xlsx.rs` | calamine: đọc MỌI sheet (xls/xlsb/ods) |
| `src/conv/pptx.rs` | zip + quick-xml: slide sort theo số thứ tự |
| `src/conv/html.rs` | htmd (skip script/style/noscript) |
| `src/conv/csv_conv.rs` | csv: strip BOM, TCVN3 fallback, sniff delimiter, chứa `rows_to_md_table` chung |
| `src/proc.rs` | `background_command()`: CREATE_NO_WINDOW trên Windows (tránh flash console) |
| `src/image_ocr.rs` | Tesseract CLI + tiền xử lý ảnh (grayscale/upscale/unsharpen/normalize); dùng `proc::background_command` |
| `src/audio.rs` | AudioEngine (cache Whisper), decode symphonia + resample 16k, lang "vi" |
| `src/chunk.rs` | tách chunk RAG theo heading-path |
| `src/viet_legacy.rs` | decode TCVN3/VNI/VPS; opt-in `Tcvn3CaseHint` (TCVN3/ABC all-capital H-font) — TXT/CSV không suy hoa |
| `src/llm.rs` | (feature `llm`) chat/summarize/extract_json/vision_ocr qua env `FILECONV_LLM_*` |
| `src/probe.rs` | `probe()` → FileInfo{format,bytes,pages,sheets} |
| `src/tables.rs` | `tables_json` (xlsx/csv → JSON rows) — LƯU Ý: không decode TCVN3 như csv_conv.rs |

## `crates/cli/` — binary `fileconv`

| File | Trách nhiệm |
|---|---|
| `src/main.rs` | dispatch 4 subcommand: `one` / `speed` / `accuracy` / `audio`; panic hook |
| `src/metrics.rs` | CER/WER Levenshtein + `normalize()` (bỏ markdown để đo NỘI DUNG) |

## `crates/mcp/` — binary `fileconv-mcp`

| File | Trách nhiệm |
|---|---|
| `src/main.rs` | server stdio (rmcp); 8 tool (4 all-deterministic + 4 LLM) |
| `README.md` | cách đăng ký `claude mcp add fileconv ...` |

## `app/` — desktop "Markhand" (Tauri 2)

| Vùng | File chính | Trách nhiệm |
|---|---|---|
| entry | `src/main.tsx`, `src/App.tsx` | root + drag-drop toàn cửa sổ + toast lỗi |
| components | `Sidebar.tsx`, `Tree.tsx` | cây file, toolbar upload/tạo, đổi DATA root |
| | `DocView.tsx` | workspace 3 tab (split/md/source), Save/Copy/Reconvert |
| | `MarkdownEditor.tsx` | CodeMirror (Soạn) + react-markdown (Xem trước) |
| | `SourcePreview.tsx` | dispatch theo format: Pdf/Docx/Excel/Text/Image/Audio + size-gate |
| | `Settings.tsx` | modal cài OCR lang, PDF OCR, audio lang/threads, model whisper |
| state | `state/store.ts` | Zustand store duy nhất (không persist — Rust là nguồn sự thật) |
| lib | `lib/ipc.ts`, `lib/types.ts` | wrap `invoke` + type `FsNode`/`Settings` mirror Rust serde |
| Tauri bridge | `src-tauri/src/lib.rs` | 18 `#[tauri::command]`, AppState, `convert_and_write_md` |
| | `src-tauri/src/main.rs` | shim 6 dòng gọi `run()` |
| config | `src-tauri/tauri.conf.json`, `capabilities/default.json` | identity Markhand, permission tối thiểu |

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
| Thêm / sửa định dạng | `crates/core/src/conv/<fmt>.rs` + định tuyến ở `lib.rs` |
| Đổi tiền xử lý OCR | `crates/core/src/image_ocr.rs` |
| Đổi phụ thuộc native OCR (Tesseract/whisper) | `image_ocr.rs` / `audio.rs` |
| Spawn subprocess (CLI, OCR, LLM) | dùng `crate::proc::background_command` chứ không `Command::new` (tránh console flash Windows) |
| Thêm CLI flag | `crates/cli/src/main.rs` |
| Thêm MCP tool | `crates/mcp/src/main.rs` (+ `crates/core/src/llm.rs` nếu cần LLM) |
| Sửa GUI | `app/src/components/*.tsx` |
| Sửa cầu nối Tauri | `app/src-tauri/src/lib.rs` |
| Đo lại sau đổi | `bench/` + `fileconv speed`/`accuracy` |

## Tham chiếu chéo
- Quy ước & cạm bẫy khi sửa: [`code-standards.md`](code-standards.md)
- Luồng kiến trúc: [`system-architecture.md`](system-architecture.md)
