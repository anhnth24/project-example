# fileconv-mcp

MCP server (stdio) để AI/agent chuyển file → Markdown **offline, không cần API key**.
Tái dùng `fileconv-core`. Giúp agent đọc file không-phải-md (pdf/docx/pptx/xlsx/csv/html/ảnh/audio)
mà **đỡ tốn token**: trích xuất ngoài context + chỉ lấy phần cần.

## Tool

| Tool | Mô tả |
|---|---|
| `detect_format(path)` | Xem loại/kích thước/số trang/sheet — KHÔNG convert (rẻ, ~0 token) |
| `convert_to_markdown(path, {pages?, sheet?, max_chars?, ocr_langs?})` | Convert; `pages` (PDF, 1-indexed), `sheet` (Excel), `max_chars` để chỉ lấy phần cần |

## Build

```bash
cargo build --release -p fileconv-mcp   # ra target/release/fileconv-mcp
```

## Đăng ký với Claude Code

```bash
claude mcp add fileconv -- /đường/dẫn/target/release/fileconv-mcp
```

Tài nguyên native (tuỳ chọn) trỏ qua env khi đăng ký:
`FILECONV_PDFIUM_LIB` (PDF), `FILECONV_TESSDATA` (OCR chất lượng cao), `FILECONV_WHISPER_MODEL` (audio).
Thiếu thì tự fallback / báo lỗi rõ.

## Ví dụ luồng agent (tiết kiệm token)

1. `detect_format("bao_cao.pdf")` → `{ "pages": 96 }`
2. `convert_to_markdown("bao_cao.pdf", { "pages": [1,2,3] })` → chỉ 3 trang đầu.

> Phần tóm tắt / trích JSON ngữ nghĩa: để agent tự làm trên Markdown trả về (agent đã là LLM),
> hoặc bật lớp LLM tuỳ chọn (có API key) ở phiên bản sau.
