# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Dự án

Backend Rust chuyển đổi file (pdf/docx/pptx/xlsx/csv/html + ảnh OCR + audio) → Markdown,
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

- `tesseract-ocr` + `tesseract-ocr-vie` (CLI) — OCR ảnh. Gọi qua `std::process::Command`.
- **libpdfium**: `bash bench/download_pdfium.sh` → `./pdfium/lib`. Thiếu thì PDF tự fallback `pdf-extract`.
- **tessdata_best** (khuyến nghị): `bash bench/download_tessdata.sh` → `./tessdata_best`.
  Backend tự dùng nếu có (chính xác hơn với tài liệu thật/IN HOA), thiếu thì dùng model nhẹ hệ thống.
- **model whisper**: `bash bench/download_models.sh` → `./models/ggml-{tiny,base,small}.bin`
  + **ggml-PhoWhisper-small.bin** (VinAI fine-tune tiếng Việt — đo được 90.8% vs 77.3%
  whisper-small cùng cỡ trên corpus vi; license PhoWhisper chưa rõ, kiểm tra trước khi phân phối).
- Build whisper-rs cần cmake + C/C++ + clang (bindgen). Lần build đầu compile whisper.cpp (~1-2 phút).

Đường dẫn override qua env: `FILECONV_PDFIUM_LIB`, `FILECONV_TESSDATA`.
Thư mục tải về (`pdfium/`, `tessdata_best/`, `models/`, `bench/corpus*`, `bench/edge`) đều gitignore.

## Kiến trúc

- **`crates/core`** (`fileconv-core`) — lõi convert. `Converter::convert_path(&Path)` định
  tuyến theo `FormatKind` (suy từ đuôi file, KHÔNG sniff magic-byte) tới module trong `conv/`.
  Mỗi converter có `to_markdown(...) -> Result<String, ConvertError>`, **gọi thẳng crate gốc**:
  - `conv/pdf.rs` — **`pdf-inspector` (chính)**: markdown CÓ CẤU TRÚC theo từng trang
    (heading theo cỡ chữ, bảng, sắp lại đa cột) + cờ `needs_ocr` (bắt cả text-layer rác/font GID).
    Trang `needs_ocr` → render PDFium 300 DPI + OCR Tesseract (pdf-inspector không OCR).
    Fallback: PDFium đếm ký tự → `pdf-extract`. PDFium cache 1 instance/thread (`thread_local`,
    chỉ init 1 lần/tiến trình). `pdf_ocr_images` (mặc định tắt) OCR thêm ảnh nhúng cho trang trộn.
    Đánh đổi: pdf-inspector ~35ms/trang (vs PDFium ~6ms) nhưng cho cấu trúc + đa cột.
  - `conv/xlsx.rs` — `calamine` (đọc MỌI sheet, hỗ trợ cả `.xls`).
  - `conv/docx.rs` — `docx-rust`: duyệt từng run, xử lý `<w:br>/<w:tab>` (tránh dính chữ),
    phát hiện heading qua style.
  - `conv/pptx.rs` — `zip`+`quick-xml`, đọc slide đúng thứ tự số.
  - `conv/html.rs` — `htmd` (đã `skip_tags` script/style/noscript; thay html2md vì nó phình output).
  - `conv/csv_conv.rs` — bảng Markdown; `rows_to_md_table()` dùng chung cho xlsx/docx.
  - `image_ocr.rs` — Tesseract CLI. `ocr_dynimage()` là đường chung: **tiền xử lý ảnh**
    (grayscale → upscale ×2 nếu nhỏ → unsharpen → normalize); output nghi lỗi/dính
    IN HOA retry PSM 6 và chọn theo quality score.
  - `audio.rs` — `AudioEngine::load()` giữ `WhisperContext` (cache 1 lần); decode mp3/wav/ogg…
    bằng `symphonia` + resample 16kHz, phiên âm whisper-rs (lang "vi"), lọc
    `no_speech_probability`. Tự tìm PhoWhisper đã tải về trước model chuẩn.
  - `chunk.rs` — chia Markdown thành chunk RAG theo heading (giữ đường dẫn tiêu đề cha).
  - `viet_legacy.rs` — decode bảng mã VN cũ **TCVN3** (detect + convert; VNI/VPS backlog).
  - `llm.rs`/`llm_cli.rs` (feature `llm`) — HTTP chat/vision, neural embedding và
    Cursor/Codex official subscription CLI; Claude subscription không route qua app thứ ba.
  - Output cuối `convert_path` luôn **chuẩn hoá NFC** (tài liệu vi NFD từ macOS/PDF cũ).
- **`crates/cli`** (`fileconv`) — bench harness: timing, đếm page (pdfinfo/python zip),
  CER/WER (`metrics.rs`, Levenshtein; `normalize()` bỏ ký hiệu markdown để đo NỘI DUNG).
- **`bench/`** — script tải corpus thật + sinh dữ liệu ground-truth tiếng Việt + các báo cáo
  (`REPORT*.md`). `ocr_experiment.py`/`paddle_test.py` là tư liệu thí nghiệm chất lượng OCR.

## Lưu ý khi sửa code

- Pin có chủ đích: `pdf-extract =0.8.2` (0.12 panic), `symphonia 0.5` (0.6 đổi API). Đừng nâng bừa.
- PDF/whisper resource đắt → giữ pattern cache (thread_local PDFium, OnceLock AudioEngine trong `Converter`).
- Khi đổi OCR/PDF, **đo lại** bằng `fileconv accuracy`/`speed` trên corpus (tái tạo bằng `bench/*.sh`).
- Điểm yếu đã biết (xem `bench/REPORT*.md`): IN HOA dính chữ, bảng PDF nhiều cột, whisper ảo giác
  audio không lời, chữ viết tay. Tài liệu khó → tier vision-LLM: MCP tool `ocr_hard` (cần key).
- Model identifier không đưa vào commit/code.
