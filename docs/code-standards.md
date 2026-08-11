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
| `sha2` (workspace, `sha2 = "=0.11.0"`) | `=0.11.0` | contract durable intelligence ID `sha256-v1` (ADR 0013): `DefaultHasher` không ổn định qua version Rust nên ID SQLite/HNSW/handoff sẽ trôi; pin để cùng họ crate với ADR 0006 (server digest) — đừng nâng ngoài quy trình ADR |

Crate chính (không pin cứng nhưng cố ý giữ ổn định): `pdfium-render 0.9.2`, `pdf-inspector 0.1.3`,
`docx-rust 0.1.11`, `whisper-rs 0.16`, `htmd 0.5`, `calamine 0.35`, `quick-xml 0.37`, `zip 2.2`,
`csv 1.3`, `image 0.25`, `unicode-normalization 0.1.25`.

## Cache pattern — MUST bảo toàn

PDF và whisper **đắt** → phải giữ pattern cache. Đừng "dọn" thành gọi thẳng mỗi lần.

- **PDFium thread_local + PDFIUM_CALL lock** (`crates/core/src/conv/pdf/pdfium.rs`): `thread_local! { static PDFIUM: Option<Pdfium> = load_pdfium() }`.
  Mỗi thread 1 instance, init 1 lần/tiến trình. Chỉ load khi thực sự cần OCR (`need_pdfium` gate).
  **libpdfium KHÔNG thread-safe**: mỗi region dùng PDFium (cả render+OCR) phải acquire `PDFIUM_CALL: Mutex<()>` trước.
  Concurrent scanned-PDF conversions sẽ queue tại lock (trade-off vs throughput).
  Đường dẫn lib qua env `FILECONV_PDFIUM_LIB` → `pdfium/lib/*` → thư viện hệ thống.
- **Whisper process-wide LRU cache** (`crates/core/src/audio.rs`): `LoadOnceCache` keyed by
  `WhisperModelKey` (canonical model path + immutable load knobs: `use_gpu`/`flash_attn`/`gpu_device`).
  Per-key state is `Loading | Ready | Failed` with a condvar (fail/retry never overlaps loaders).
  Ready set is LRU-bounded (default 2, override `FILECONV_WHISPER_CACHE_CAPACITY`); eviction drops
  the cache entry while outstanding `AudioEngine` `Arc`s keep in-flight contexts alive. Production
  loader derives all behavior from the complete key (no public injectable loader). Runtime knobs
  (`audio_threads`, `audio_no_speech_threshold`) stay on `AudioEngine` and are **not** part of the
  cache key. Resample to 16 kHz uses `rubato` FFT with partial/flush + `output_delay` trim;
  returns `Result` (never invents silence on failure). Trả `Unsupported` nếu chưa có model.
- **Temp file**: file tạm (ảnh render, artifact) ghi qua `tempfile::NamedTempFile`
  (exclusive `O_EXCL`/tên random) — tránh path đoán được trong `/tmp`.

## Subprocess spawning — MUST dùng `crate::proc::background_command`

GUI app (Tauri) không nên hiển thị console window khi spawn CLI subprocess (python, LLM CLI…).
Luôn dùng `crate::proc::background_command()` thay vì `Command::new()` trực tiếp:

```rust
// ✅ Đúng
let output = crate::proc::background_command("some-cli")
    .arg(&input_path)
    .arg(out_path)
    .output()?;

// ❌ Sai
let output = std::process::Command::new("some-cli")
    .arg(&input_path)
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

Sau bất kỳ thay đổi nào ở `image_ocr.rs`, `audio.rs`, `conv/pdf/`: **đo lại** bằng CLI trên corpus
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
- feature `audio` (opt-in) → `whisper-rs`/`symphonia`/`rubato`, mở transcribe. TẮT mặc định: whisper.cpp
  phải compile C++ qua cmake (~1–2 phút), chỉ CLI/desktop/MCP cần — server/knowledge dùng lõi text-only
  không gánh chi phí này.
- `cuda` / `metal` / `vulkan` / `hipblas` / `openblas` / `openmp` → **kéo theo feature `audio`** (proxy
  sang `whisper-rs` để tăng tốc GPU/BLAS), không bật độc lập được.
- `llm` → `reqwest` (blocking, rustls-tls) + `httpdate`, mở `pub mod llm`. `base64` là dependency
  **không điều kiện** (dùng cả ngoài `llm`, ví dụ `pptx_preview.rs`), không phải feature dep của `llm`.
  **MCP crate luôn build với `llm`.**

## Build native (yêu cầu môi trường)

- Build whisper-rs cần **cmake + C/C++ + clang** (bindgen). Lần đầu compile whisper.cpp ~1–2 phút.
- PDFium: `bash bench/download_pdfium.sh` → `./pdfium/lib`. Thiếu → PDF tự fallback pdf-extract.
- Whisper model: `bash bench/download_models.sh` → `./models/ggml-{tiny,base,small}.bin` + **PhoWhisper-small**.
- OCR ảnh/scan: vision-LLM (`FILECONV_OCR_API_KEY`, mặc định OpenRouter; ADR 0016 —
  Tesseract/Paddle local đã loại bỏ; self-host vision endpoint qua `FILECONV_OCR_BASE_URL`).

Override đường dẫn qua env: `FILECONV_PDFIUM_LIB`, `FILECONV_WHISPER_MODEL`.

## Cạm bẫy đã biết (tránh lặp)

- `pdf-extract` và `pdf-inspector` đều bọc `catch_unwind(AssertUnwindSafe)` — lopdf/pdf-extract panic trên PDF malformed. Giữ wrapper.
- `tables.rs` CSV path dùng `String::from_utf8_lossy` và **không** decode TCVN3 như `csv_conv.rs` — bất nhất đã biết.

## Tham chiếu chéo
- Map code: [`codebase-summary.md`](codebase-summary.md)
- Kiến trúc & luồng: [`system-architecture.md`](system-architecture.md)
