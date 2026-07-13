# Tiêu chuẩn code & Quy ước khi sửa

> Đọc TRƯỚC khi đụng vào code. Vi phạm các quy tắc đánh dấu **MUST** sẽ làm hỏng hiệu năng hoặc độ chính xác.

## Nguyên tắc nền

- **YAGNI → KISS → DRY** theo thứ tự. Không code phỏng đoán, không trừu tượng dùng một lần.
- **Thay đổi phẫu thuật**: chỉ đụng những gì cần. Không "tốt lên" code/sidebar/format kề cạnh.
- Hành vi thật, không mock/fake dữ liệu để qua cổng kiểm tra.
- Commit theo conventional format, **không nhắc AI** trong commit/code.

## Pin có chủ đích — KHÔNG nâng bừa

| Crate | Pin | Lý do |
|---|---|---|
| `pdf-extract` | `=0.8.2` | 0.12 **panic** trên một số PDF mà 0.8.2 xử lý được |
| `symphonia` | `0.5` | 0.6 cấu trúc lại module, đổi API |
| `time` (app) | `=0.3.51` | tương thích cookie 0.18.1 |

Crate chính (không pin cứng nhưng cố ý giữ ổn định): `pdfium-render 0.9.2`, `pdf-inspector 0.1.3`,
`docx-rust 0.1.11`, `whisper-rs 0.16`, `htmd 0.5`, `calamine 0.35`, `quick-xml 0.37`, `zip 2.2`,
`csv 1.3`, `image 0.25`, `unicode-normalization 0.1.25`.

## Cache pattern — MUST bảo toàn

PDF và whisper **đắt** → phải giữ pattern cache. Đừng "dọn" thành gọi thẳng mỗi lần.

- **PDFium thread_local + PDFIUM_CALL lock** (`crates/core/src/conv/pdf.rs`): `thread_local! { static PDFIUM: Option<Pdfium> = load_pdfium() }`.
  Mỗi thread 1 instance, init 1 lần/tiến trình. Chỉ load khi thực sự cần OCR (`need_pdfium` gate).
  **libpdfium KHÔNG thread-safe**: mỗi region dùng PDFium (cả render+OCR) phải acquire `PDFIUM_CALL: Mutex<()>` trước.
  Concurrent scanned-PDF conversions sẽ queue tại lock (trade-off vs throughput).
  Đường dẫn lib qua env `FILECONV_PDFIUM_LIB` → `pdfium/lib/*` → thư viện hệ thống.
- **AudioEngine OnceLock** (`crates/core/src/lib.rs`): `Converter.engine: OnceLock<AudioEngine>`,
  lazy load model Whisper GGML **một lần** mỗi `Converter`. Gọi audio sau dùng lại. Trả `Unsupported`
  nếu chưa set `whisper_model`. (Có race benign được tài liệu hoá: nếu thread khác set trước, `set` bị bỏ qua.)
- **Tesseract**: spawn mỗi lần qua `crate::proc::background_command()` (không cache process). Temp PNG dùng bộ đếm `AtomicU64`.

## Subprocess spawning — MUST dùng `crate::proc::background_command`

GUI app (Tauri) không nên hiển thị console window khi spawn CLI subprocess (tesseract, python, LLM CLI).
Luôn dùng `crate::proc::background_command()` thay vì `Command::new()` trực tiếp:

```rust
// ✅ Đúng
let output = crate::proc::background_command("tesseract")
    .arg(&image_path)
    .arg(out_path)
    .output()?;

// ❌ Sai
let output = std::process::Command::new("tesseract")
    .arg(&image_path)
    .arg(out_path)
    .output()?;
```

`background_command()` tự động thêm `CREATE_NO_WINDOW` flag trên Windows. stdout/stderr capture không đổi.

## NFC — MUST trên mọi output

Mọi output của `convert_path` phải chuẩn hoá NFC (`unicode_normalization`, có `is_nfc_quick` guard
tránh chuẩn hoá lại text đã NFC). Tài liệu VN thường dính NFD từ macOS/PDF cũ → bắt buộc để chữ đúng.

## Định tuyến theo đuôi file, KHÔNG sniff magic-byte

`FormatKind::from_path` match **extension** lowercase. Đừng thêm sniff magic-byte — sẽ phá contract
và cách app/CLI gom file.

## Khi đổi OCR / PDF — MUST đo lại

Sau bất kỳ thay đổi nào ở `image_ocr.rs`, `audio.rs`, `conv/pdf.rs`: **đo lại** bằng CLI trên corpus
(tái tạo qua `bench/*.sh`):

```bash
./target/release/fileconv speed   bench/corpus        bench/REPORT_SPEED.md
./target/release/fileconv accuracy bench/vn_corpus/manifest.tsv bench/REPORT_ACCURACY.md
```

Không đo lại = không claims "nhanh/đúng hơn". Quy tắc Fail loud: báo rõ nếu bỏ qua bước đo.

## `vendor/markitdown-rs/` — tham khảo, KHÔNG phụ thuộc

Đã `exclude` khỏi workspace. Nếu cần ý tưởng thì đọc, nhưng **không** `use`/path-dep/import từ đó.

## Quy ước riêng từng vùng

- **Rust**: snake_case (`fn`, `mod`), PascalCase (`struct`/`enum`), crate gốc gọi trực tiếp trong `conv/*`.
  Module `conv::*` là **private** — caller chỉ đi qua `Converter::convert_path`.
- **TypeScript (app)**: strict mode, `noUnusedLocals/Parameters`. File `.ts`/`.tsx` kebab-case không bắt buộc
  nhưng komponent PascalCase. State qua Zustand store duy nhất (`state/store.ts`), **không** persist (Rust là nguồn sự thật).
- **Python (bench)**: snake_case. Chỉ để sinh corpus + thí nghiệm, không phải production code.
- **Comment / UI string**: tiếng Việt (theo convention dự án). Giải thích "tại sao", không lặp "cái gì".

## Tính năng (feature gates)

- `default = []` — core build tinh gọn, offline.
- `cuda` / `metal` / `vulkan` / `hipblas` / `openblas` / `openmp` → proxy sang `whisper-rs` để tăng tốc GPU.
- `llm` → `reqwest` (blocking, rustls-tls) + `base64`, mở `pub mod llm`. **MCP crate luôn build với `llm`.**

## Build native (yêu cầu môi trường)

- Build whisper-rs cần **cmake + C/C++ + clang** (bindgen). Lần đầu compile whisper.cpp ~1–2 phút.
- PDFium: `bash bench/download_pdfium.sh` → `./pdfium/lib`. Thiếu → PDF tự fallback pdf-extract.
- tessdata_best (khuyến nghị, tài liệu thật/IN HOA): `bash bench/download_tessdata.sh` → `./tessdata_best`.
- Whisper model: `bash bench/download_models.sh` → `./models/ggml-{tiny,base,small}.bin` + **PhoWhisper-small**.
- Tesseract: cài `tesseract-ocr` + `tesseract-ocr-vie` (CLI).

Override đường dẫn qua env: `FILECONV_PDFIUM_LIB`, `FILECONV_TESSDATA`, `FILECONV_WHISPER_MODEL`.

## Cạm bẫy đã biết (tránh lặp)

- `pdf-extract` và `pdf-inspector` đều bọc `catch_unwind(AssertUnwindSafe)` — lopdf/pdf-extract panic trên PDF malformed. Giữ wrapper.
- `tables.rs` CSV path dùng `String::from_utf8_lossy` và **không** decode TCVN3 như `csv_conv.rs` — bất nhất đã biết.
- CLI `count_pages` cho PPTX shell ra `python3` trong khi `probe.rs` đếm native Rust — logic trùng, 2 implementation.

## Tham chiếu chéo
- Map code: [`codebase-summary.md`](codebase-summary.md)
- Kiến trúc & luồng: [`system-architecture.md`](system-architecture.md)
