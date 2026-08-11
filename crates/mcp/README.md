# fileconv-mcp

MCP server (stdio) để AI/agent chuyển file → Markdown, chạy local và **đỡ tốn
token**: trích xuất ngoài context + chỉ lấy phần cần. Tái dùng `fileconv-core`.
Văn bản số (pdf text-layer/docx/pptx/xlsx/csv/html) convert tất định không cần
API key; riêng **ảnh và PDF scan cần OCR đi qua vision-LLM** (`FILECONV_OCR_*`,
ADR 0016 — Tesseract local đã bị loại bỏ), thiếu key sẽ báo `DependencyMissing`
rõ ràng.

## Tool

**Không cần API key (tất định — trừ ảnh/PDF scan cần `FILECONV_OCR_*`):**

| Tool | Mô tả |
|---|---|
| `detect_format(path)` | Loại/kích thước/số trang/sheet — KHÔNG convert (rẻ, ~0 token) |
| `convert_to_markdown(path, {pages?, sheet?, max_chars?, ocr_langs?})` | Convert; `pages` (PDF, 1-indexed), `sheet` (Excel), `max_chars` để chỉ lấy phần cần |
| `extract_tables_json(path, {sheet?})` | Excel/CSV → JSON rows |
| `convert_chunks(path, {max_chars?})` | Convert + chia **chunk cho RAG** theo heading (giữ đường dẫn tiêu đề, vd "Chương I > Điều 1") |

**Cần cấu hình LLM (`FILECONV_LLM_*`) — chưa cấu hình sẽ báo lỗi rõ:**

| Tool | Mô tả |
|---|---|
| `summarize(path, {max_chars?})` | Tóm tắt tài liệu |
| `extract_json(path, {instruction, max_chars?})` | Trích trường ngữ nghĩa → JSON (vd hoá đơn/HĐ) |
| `translate(path, {target, max_chars?})` | Dịch sang ngôn ngữ đích |
| `ocr_hard(path)` | OCR ảnh bằng **vision-LLM** với provider/model tuỳ chọn (`FILECONV_LLM_*`) — convert thường đã OCR vision qua `FILECONV_OCR_*` |

### Cấu hình LLM (tuỳ chọn)
```
FILECONV_LLM_PROVIDER = openai | anthropic | gemini | openai-compatible
FILECONV_LLM_API_KEY  = <key>
FILECONV_LLM_MODEL    = <model>     # tuỳ chọn (có mặc định)
FILECONV_LLM_BASE_URL = <url>       # tuỳ chọn — ollama/openrouter/local
```
Không cấu hình → chỉ dùng nhóm tool tất định phía trên (mặc định). Bật LLM = nội dung được
gửi tới nhà cung cấp tương ứng (cân nhắc riêng tư). Build cần feature `llm` (đã bật sẵn cho crate mcp).

## Build

```bash
cargo build --release -p fileconv-mcp   # ra target/release/fileconv-mcp
```

## Đăng ký với Claude Code

```bash
claude mcp add fileconv -- /đường/dẫn/target/release/fileconv-mcp
```

Tài nguyên native (tuỳ chọn) trỏ qua env khi đăng ký:
`FILECONV_PDFIUM_LIB` (PDF render), `FILECONV_WHISPER_MODEL` (audio).
OCR ảnh/scan cấu hình qua `FILECONV_OCR_API_KEY`/`FILECONV_OCR_BASE_URL`/
`FILECONV_OCR_MODEL` (mặc định OpenRouter `qwen/qwen3.7-flash`; endpoint vision
self-host dùng được khi có GPU). Thiếu thì tự fallback / báo lỗi rõ.

## Ví dụ luồng agent (tiết kiệm token)

1. `detect_format("bao_cao.pdf")` → `{ "pages": 96 }`
2. `convert_to_markdown("bao_cao.pdf", { "pages": [1,2,3] })` → chỉ 3 trang đầu.

> Phần tóm tắt / trích JSON ngữ nghĩa: để agent tự làm trên Markdown trả về (agent đã là LLM),
> hoặc bật lớp LLM tuỳ chọn (có API key) ở phiên bản sau.
