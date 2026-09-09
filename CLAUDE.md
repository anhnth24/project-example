# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Dự án

Backend Rust chuyển đổi file (pdf/docx/pptx/xlsx/csv/html/txt + ảnh OCR + audio) → Markdown,
**code do dự án làm chủ hoàn toàn**. `vendor/markitdown-rs/` chỉ là **tài liệu tham khảo**
(MIT) — KHÔNG phải dependency, đã `exclude` khỏi workspace; đừng dùng lại hay phụ thuộc nó.
Desktop Tauri Markhand đã có bundle config; `.deb` Linux build được, Win/Mac còn
cần runner + signing/notarization.

Ưu tiên xuyên suốt: **độ chính xác nội dung tiếng Việt** > giữ format 100%.

## Lệnh hay dùng

```bash
cargo build --release                          # build (release để đo tốc độ đúng)
cargo test                                     # test (unit test CER/WER nằm ở crates/cli)
cargo test -p fileconv-cli metrics             # chạy nhóm test metrics

./target/release/fileconv one <file>           # convert 1 file → stdout
./target/release/fileconv speed <dir> [out.md] # benchmark tốc độ (ms/file, ms/page)
./target/release/fileconv accuracy <manifest.tsv> [out.md]   # CER/WER vs ground-truth
./target/release/fileconv audio <model1,model2> <manifest.tsv> [out.md]  # WER/RTF whisper
```

Manifest accuracy/audio: mỗi dòng `<file>\t<ground_truth.txt>\t<nhãn>` (đường dẫn tương
đối tính theo thư mục manifest). `#` đầu dòng là comment.

## Phụ thuộc native bên ngoài (cần cài/tải để chạy đầy đủ)

- **Vision OCR (OpenRouter mặc định)** — OCR ảnh/trang scan qua API OpenAI-compatible
  (feature `llm`, mặc định bật ở CLI). Env: `FILECONV_OCR_API_KEY` (fallback
  `FILECONV_LLM_API_KEY`), `FILECONV_OCR_BASE_URL`, `FILECONV_OCR_MODEL`,
  `FILECONV_OCR_SYSTEM_PROMPT`, `FILECONV_OCR_TIMEOUT_SECS`. Tesseract/Paddle local
  đã bị **loại bỏ hoàn toàn** (quyết định product 2026-08-10); thiếu key → lỗi
  `DependencyMissing` rõ ràng.
- **libpdfium**: `bash bench/download_pdfium.sh` → `./pdfium/lib`. Thiếu thì PDF tự fallback `pdf-extract`.
- **model whisper**: `bash bench/download_models.sh` → `./models/ggml-{tiny,base,small}.bin`
  + **ggml-PhoWhisper-small.bin** (VinAI fine-tune tiếng Việt — đo được 90.8% vs 77.3%
  whisper-small cùng cỡ trên corpus vi; license PhoWhisper chưa rõ, kiểm tra trước khi phân phối).
- Build whisper-rs cần cmake + C/C++ + clang (bindgen). Lần build đầu compile whisper.cpp (~1-2 phút).

Đường dẫn override qua env: `FILECONV_PDFIUM_LIB`.
Thư mục tải về (`pdfium/`, `models/`, `bench/corpus*`, `bench/edge`) đều gitignore.

## Kiến trúc

- **`crates/core`** (`fileconv-core`) — lõi convert. `Converter::convert_path(&Path)` định
  tuyến theo `FormatKind` (suy từ đuôi file, KHÔNG sniff magic-byte) tới module trong `conv/`.
  Mỗi converter có `to_markdown(...) -> Result<String, ConvertError>`, **gọi thẳng crate gốc**:
  - `conv/pdf.rs` — **`pdf-inspector` (chính)**: markdown CÓ CẤU TRÚC theo từng trang
    (heading theo cỡ chữ, bảng, sắp lại đa cột) + cờ `needs_ocr` (bắt cả text-layer rác/font GID).
    Trang `needs_ocr` → render PDFium 300 DPI + OCR vision-LLM (pdf-inspector không OCR).
    Fallback: PDFium đếm ký tự → `pdf-extract`. PDFium cache 1 instance/thread (`thread_local`,
    chỉ init 1 lần/tiến trình). `pdf_ocr_images` (mặc định tắt) OCR thêm ảnh nhúng cho trang trộn.
    Đánh đổi: pdf-inspector ~35ms/trang (vs PDFium ~6ms) nhưng cho cấu trúc + đa cột.
  - `conv/xlsx.rs` — `calamine` (mọi sheet, xls/xlsb/ods); merge/multiline → HTML table.
  - `conv/docx.rs` — `docx-rust`: duyệt từng run, xử lý `<w:br>/<w:tab>` (tránh dính chữ),
    phát hiện heading qua style.
  - `conv/pptx.rs` — text Markdown; `pptx_preview.rs` parse vị trí text/ảnh/shape cho desktop SVG.
  - `conv/html.rs` — `htmd` (đã `skip_tags` script/style/noscript; thay html2md vì nó phình output).
  - `conv/csv_conv.rs` — Markdown table hoặc sanitized HTML rowspan/colspan.
  - `image_ocr.rs` — vision-LLM OCR (OpenRouter/OpenAI-compatible): decode có
    `Limits` chống bomb → thu về ≤2400px cạnh dài → JPEG q90 → gửi provider với
    system prompt "chép trung thực" (giữ số hiệu/điều khoản/bảng/công thức,
    `[không rõ: ...]` khi không chắc); strip code fence bao ngoài. Thiếu
    key/feature `llm` → `DependencyMissing`, không âm thầm bỏ trang.
  - `audio.rs` — `AudioEngine::load()` lấy `WhisperContext` từ cache **process-wide LRU**
 (key: canonical path + load knobs; Loading/Ready/Failed + condvar; capacity mặc định 2 /
 `FILECONV_WHISPER_CACHE_CAPACITY`); decode mp3/wav/ogg… bằng `symphonia` + resample 16kHz
 qua `rubato` FFT (trims delay), phiên âm whisper-rs (lang "vi"), lọc
 `no_speech_probability`. Tự tìm PhoWhisper đã tải về trước model chuẩn.
  - `chunk.rs` — chia Markdown thành chunk RAG theo heading (giữ đường dẫn tiêu đề cha).
  - `viet_legacy.rs` — decode **TCVN3, VNI-Windows, VPS**; maps sinh từ VietUnicode;
    opt-in `Tcvn3CaseHint::UppercaseFont` khi caller có metadata TCVN3/ABC all-capital H-font
    (không đoán từ TXT/CSV; helper font yêu cầu prefix `.Vn`).
  - `llm.rs`/`llm_cli.rs` (feature `llm`) — HTTP chat/vision, neural embedding và
    Cursor/Codex official subscription CLI; Claude subscription không route qua app thứ ba.
  - Desktop RAG: SQLite FTS5 + local/provider vector + persistent HNSW cache; exact fallback.
  - Desktop watch: `notify` recursive/debounce, cấm watch trong DATA để tránh loop.
  - Output cuối `convert_path` luôn **chuẩn hoá NFC** (tài liệu vi NFD từ macOS/PDF cũ).
- **`crates/cli`** (`fileconv`) — bench harness: timing, đếm page (pdfinfo/native probe),
  CER/WER (`metrics.rs`, Levenshtein; `normalize()` bỏ ký hiệu markdown để đo NỘI DUNG).
- **`crates/knowledge`** (`fileconv-knowledge`) — knowledge extraction & retrieval contracts,
  chia sẻ các kiểu dữ liệu và ranh giới embedding giữa Desktop RAG và Markhand Web Server.
- **`crates/server`** (`fileconv-server`) — backend Web API & background worker (Compose stack:
  PostgreSQL, Qdrant, MinIO) xử lý chuyển đổi, đánh chỉ mục và tìm kiếm ngữ nghĩa.
- **`crates/mcp`** (`fileconv-mcp`) — stdio MCP server cung cấp 9 công cụ cho Claude Code
  (convert, format probe, table extract, chunking, summarize, JSON extract, translation, vision OCR hard).
- **`bench/`** — script tải corpus thật + sinh dữ liệu ground-truth tiếng Việt + các báo cáo
  (`REPORT*.md`). `ocr_experiment.py`/`paddle_test.py` là tư liệu thí nghiệm chất lượng OCR.

## Hướng dẫn gỡ lỗi & Cạm bẫy thường gặp (Debugging & Common Pitfalls)

1. **Lỗi thiếu PDFium (`DependencyMissing` hoặc fallback chậm)**:
   - Nếu chạy trên Linux/macOS/Windows mà không tìm thấy lib PDFium, PDF scan sẽ báo lỗi hoặc PDF text fallback về `pdf-extract` chậm hơn và dễ mất cấu trúc.
   - **Khắc phục**: Chạy `bash bench/download_pdfium.sh` để tải thư viện vào `./pdfium/lib`, hoặc thiết lập biến môi trường `export FILECONV_PDFIUM_LIB=/path/to/pdfium/lib`. Kiểm tra nhanh bằng `./target/release/fileconv info`.

2. **Lỗi Whisper linking / Model không tìm thấy**:
   - Khi build lần đầu với feature `audio`, `whisper.cpp` yêu cầu CMake + C/C++ compiler + Clang (bindgen). Trên Linux cần GNU toolchain để link đúng `libstdc++`.
   - Nếu chạy lệnh `audio` bị lỗi thiếu model, hãy chạy `bash bench/download_models.sh` để tải `ggml-base.bin`, `ggml-small.bin`, `ggml-PhoWhisper-small.bin` vào thư mục `models/`. Hoặc chỉ định rõ qua `FILECONV_WHISPER_MODEL`.

3. **Lỗi Vision OCR thiếu API Key (`DependencyMissing`)**:
   - Tesseract và Paddle OCR local đã được loại bỏ hoàn toàn theo ADR 0016. Khi convert ảnh hoặc PDF scan, hệ thống mặc định gọi vision API qua OpenRouter.
   - **Khắc phục**: Thiết lập `export FILECONV_OCR_API_KEY=sk-or-...`. Nếu tự host vLLM/Ollama vision, đặt thêm `FILECONV_OCR_BASE_URL` và `FILECONV_OCR_MODEL`.

4. **Lỗi panic / dính chữ trên bảng mã cũ (TCVN3/VNI/VPS)**:
   - Các font chữ hoa TCVN3 (như `.VnTimeH`) cần opt-in hint `Tcvn3CaseHint::UppercaseFont` từ metadata font, không được tự động đoán hoa/thường từ chuỗi thô TXT/CSV để tránh sai lệch nghĩa tiếng Việt.

## Lưu ý khi sửa code

- Pin có chủ đích: `pdf-extract =0.8.2` (0.12 panic), `symphonia 0.5` (0.6 đổi API), `sha2 = "=0.11.0"`. Đừng nâng bừa.
- PDF/whisper resource đắt → giữ pattern cache (thread_local PDFium, process-wide Whisper
  LRU `LoadOnceCache` trong `audio.rs` — không reload model mỗi `Converter`/request; eviction
  không unbounded).
- Khi đổi OCR/PDF, **đo lại** bằng `fileconv accuracy`/`speed` trên corpus (tái tạo bằng `bench/*.sh`).
- Điểm yếu đã biết (xem `bench/REPORT*.md`): IN HOA dính chữ, bảng PDF nhiều cột, whisper ảo giác
  audio không lời, chữ viết tay. Tài liệu khó → tier vision-LLM: MCP tool `ocr_hard` (cần key).
- Model identifier không đưa vào commit/code.
