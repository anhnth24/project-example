# Tổng quan dự án & Yêu cầu phát triển sản phẩm (PDR)

> Tài liệu nguồn sự thật về **mục đích, phạm vi và yêu cầu** của dự án.
> Hướng dẫn nhanh cho agent xem [`../CLAUDE.md`](../CLAUDE.md). Kiến trúc chi tiết xem [`system-architecture.md`](system-architecture.md).

## Dự án là gì

`fileconv` là **backend Rust** chuyển đổi tài liệu / ảnh / âm thanh sang **Markdown**,
tối ưu cho **nội dung tiếng Việt**. Code do dự án làm chủ hoàn toàn — gọi thẳng các crate
gốc thay vì bọc công cụ có sẵn. Mục tiêu cuối: đóng gói thành **desktop app (Tauri)** chạy
offline cho Win / Mac / Ubuntu.

Hai giao diện cùng dùng một lõi `fileconv-core`:
- **CLI** (`fileconv`) — convert từng file + đo tốc độ / độ chính xác (CER/WER).
- **Desktop app "Markhand"** (Tauri + React) — GUI kéo-thả, soạn thảo Markdown, xem trước nguồn.
- **MCP server** (`fileconv-mcp`) — tool cho Claude Code: convert, rút bảng, tách chunk RAG, OCR nặng qua vision-LLM.

`vendor/markitdown-rs/` **chỉ là tài liệu tham khảo (MIT)** — đã `exclude` khỏi workspace,
không phải dependency. Đừng dùng lại hay phụ thuộc nó.

## Ưu tiên xuyên suộc

> **Độ chính xác nội dung tiếng Việt > giữ format 100%.**

Khi phải đánh đổi giữa "đẹp y nguồn" và "đúng chữ tiếng Việt", chọn đúng chữ. Điều này định hình
mọi quyết định kỹ thuật: NFC bắt buộc, decode bảng mã VN cũ (TCVN3), ưu tiên PhoWhisper,
tiền xử lý ảnh trước OCR.

## Yêu cầu sản phẩm (PDR)

### Yêu cầu chức năng
| Nhóm | Yêu cầu | Trạng thái |
|---|---|---|
| Văn bản | pdf / docx / pptx / xlsx(xls/xlsb/ods) / csv / html → Markdown | ✅ |
| Ảnh | OCR tiếng Việt (Tesseract `vie+eng` + tiền xử lý ảnh) | ✅ |
| Âm thanh | Phản âm tiếng Việt (whisper-rs + symphonia, ưu tiên PhoWhisper) | ✅ |
| PDF quét | Render 300 DPI + OCR từng trang `needs_ocr` | ✅ |
| Cấu trúc | PDF có heading/bảng/đa cột (pdf-inspector) | ✅ |
| RAG | SQLite FTS5 + neural/local vectors + persistent HNSW | ✅ |
| Bảng mã cũ | Decode TCVN3, VNI-Windows và VPS | ✅ |
| OCR khó | Vision-LLM (cột nhiều, IN HOA, chữ viết tay, con dấu) qua MCP `ocr_hard` | ✅ (cần key LLM) |
| Desktop GUI | PPTX preview, merged tables, live watch, intelligence | ✅ (`.deb`; Win/Mac chờ credentials ký) |
| NFC | Chuẩn hoá mọi output (tài liệu NFD từ macOS/PDF cũ) | ✅ |

### Yêu cầu phi chức năng
- **Offline-first**: lõi + CLI + desktop chạy không cần mạng. LLM/vision chỉ là tier tuỳ chọn.
- **Hiệu năng**: tài liệu văn bản < 1ms/file; PDF ~5.7ms/trang (xem số liệu ở [`bench/REPORT.md`](../bench/REPORT.md)).
- **Đóng gói được**: chạy được trên Win/Mac/Linux chỉ với các phụ thuộc native tối thiểu.
- **Tái kiểm chứng được**: mọi thay đổi OCR/PDF phải đo lại bằng CLI trên corpus (`accuracy` / `speed`).

### Ngoài phạm vi (hiện tại)
- Đóng gói installer distributable (`.msi`/`.dmg`/`.deb`) — chưa làm.
- Nhận dạng giọng nói (ASR) streaming real-time.
- OCR chữ viết tay độ cao (giới hạn Tesseract; tier vision-LLM mới giải quyết từng phần).
- Hỗ trợ dark mode desktop (hiện ép light — xem [`system-architecture.md`](system-architecture.md#theme)).

## Động lực & vị thế thị trường

Khảo sát 11 công cụ đối thủ (chi tiết [`../bench/RESEARCH_COMPETITORS.md`](../bench/RESEARCH_COMPETITORS.md)):
- **MarkItDown**: 0.000 ở benchmark heading PDF + lỗi Unicode phi-ASCII → hợp thức hoá việc viết lại.
- **MinerU**: top OmniDocBench (95.75) nhưng cần GPU ≥16GB.
- **Marker**: nhanh (0.18s/trang H100) nhưng **license cấm công ty doanh thu >$2M**.
- **Docling**: ~6.3s/trang CPU — nặng.
- **Pandoc**: không có PDF/OCR/audio.

**Khoảng trống thị trường VN**: chưa có sản phẩm kết hợp **offline** cả (OCR vi + PDF có cấu trúc → MD + ASR vi) trên desktop. OCR vi đám mây (FPT.AI ~98%, Viettel ~99%) chỉ chạy cloud. **Nghị định 91/2025 (PDPL, hiệu lực 1/1/2026, phạt đến 5% doanh thu dữ liệu xuyên biên giới)** nghiêng về hướng offline → đúng ngách dự án.

Vị thế định vị: **"Docling offline, thu gọn, tinh chỉnh tiếng Việt, Rust, desktop + MCP."**

## Các bên liên quan & người dùng
- Người dùng cuối: người làm việc với tài liệu VN cần rút nội dung sạch (nghiên cứu, pháp lý, hành chính).
- Tích hợp: LLM/RAG qua MCP (`convert_chunks`, `extract_tables_json`).

## Tham chiếu chéo
- Cấu trúc & map code: [`codebase-summary.md`](codebase-summary.md)
- Kiến trúc: [`system-architecture.md`](system-architecture.md)
- Quy ước code: [`code-standards.md`](code-standards.md)
- Lộ trình: [`project-roadmap.md`](project-roadmap.md)
