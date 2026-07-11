# Lộ trình dự án

> Trạng thái tính đến 2026-07-10. Backlog rút từ [`../bench/RESEARCH_COMPETITORS.md`](../bench/RESEARCH_COMPETITORS.md)
> và điểm yếu đã biết trong các `REPORT*.md`.

## Đã hoàn thành ✅

- **Lõi convert** (`fileconv-core`): pdf/docx/pptx/xlsx/csv/html + ảnh OCR + audio → Markdown.
- **PDF 3-tier**: pdf-inspector (cấu trúc, đa cột, cờ `needs_ocr`) → pdfium-render → pdf-extract fallback.
- **Tiền xử lý ảnh OCR**: grayscale → upscale → unsharpen → normalize (in OCR 98.5→99.5%, low-res 81→99%).
- **NFC bắt buộc** trên mọi output (sửa tài liệu NFD từ macOS/PDF cũ).
- **Decode TCVN3** (bảng mã VN cũ) trong đường CSV/text.
- **RAG chunking** theo heading-path (`chunk.rs`).
- **CLI bench**: `one` / `speed` / `accuracy` / `audio` với CER/WER Levenshtein.
- **MCP server** (`fileconv-mcp`): 8 tool (4 deterministic + 4 LLM, gồm `ocr_hard` vision).
- **Desktop app "Markhand"** (Tauri 2 + React): kéo-thả, soạn thảo, xem trước nguồn, cài đặt OCR/audio.
- **Nghiên cứu đối thủ** (11 công cụ) — định vị ngách offline-VN.
- **Đo PhoWhisper**: 90.8% vs whisper-small 77.3% (+13.5 điểm) trên clip vi thật.
- **Document Intelligence**: persistent SQLite FTS5 + local-vector hybrid Q&A có
  citation/fallback, quality, versions/diff/merge, table/schema, PII/redaction,
  watch rules, hard-OCR hook và Knowledge Pack.
- **Handoff BA/PM**: sinh BRD/PRD, user stories, acceptance criteria, glossary,
  test cases, traceability + Jira/GitHub/Confluence/Obsidian exports.
- **Desktop LumiBase dark**: icon rail, đa tab, Library, Intelligence workspace,
  modal nội bộ và queue background.

## Đang làm / Gần ✋

- Tích hợp **PhoWhisper làm backend mặc định** cho audio vi (đã đo, chưa bật default).
- Khắc phục **bất nhất TCVN3** giữa `tables.rs` (không decode) và `csv_conv.rs` (có decode).
- Dọn **logic trùng** đếm slide PPTX (CLI shell `python3` vs `probe.rs` native).

## Backlog

### Độ chính xác tiếng Việt
- [ ] **Phục hồi dấu IN HOA**: Tesseract mất dấu ở header viết hoa (hiện giảm nhẹ bằng `tessdata_best`).
      Hướng: post-OCR phục hồi dấu + thử Vintern-1B.
- [ ] **Tách cột trước OCR** cho bảng PDF đa cột (Tesseract đang đọc sai thứ tự cột).
- [ ] **Decode VNI / VPS** đầy đủ (mới có TCVN3).
- [ ] **Lọc ảo giác whisper** trên audio không lời bằng `no_speech_probability` (hiện whisper bịa text trên nhạc/nhạc piano).

### OCR / Vision tier
- [ ] **Vintern-1B** (VLM on-device) cho tài liệu khó thay/về bên cạnh `ocr_hard` cloud.
- [ ] **PaddleOCR vi** với sắp xếp reading-order (đã có `bench/paddle_test.py` tư liệu, chưa tích hợp).
- [ ] **Chữ viết tay** — cần dữ liệu thật có nhãn (sample hiện là font-render, không phải viết tay thật; accuracy ~47.9% là giới hạn Tesseract).

### Output / cấu trúc
- [ ] **Bảng → HTML** cho ô phức tạp (merge cell, multi-line) thay vì chỉ Markdown table.
- [ ] **`ConversionResult.title`** — hiện khai báo `Option<String>` nhưng luôn `None`; chưa converter nào trả title.

### Desktop / đóng gói
- [ ] **Đóng gói distributable** (`.msi` / `.dmg` / `.deb` / AppImage) — Tauri bundler, chưa chạy.
- [x] **Dark mode** LumiBase.
- [x] Đổi `prompt()`/`confirm()` native trong Sidebar thành modal tuỳ chỉnh.
- [ ] Thống nhất identity (`package.json` `fileconv-docs` vs productName `Markhand` vs identifier `com.anhnth24.fileconv-docs`).

### Tích hợp / mở rộng
- [ ] **Plugin system** (khoảng trống vs best-in-class).
- [ ] **Benchmark tài liệu hành chính** chuyên ngành (giấy tờ, công văn) để đo độ chính xác thực tế hơn.

## Không ưu tiên (theo YAGNI)
- ASR streaming real-time.
- Đa người dùng / đồng bộ cloud (dự án định vị offline-first, đúng hướng với Nghị định 91/2025 PDPL).

## Tham chiếu chéo
- Động lực thị trường: [`project-overview-pdr.md`](project-overview-pdr.md)
- Điểm yếu chi tiết: [`../bench/REPORT_EDGE.md`](../bench/REPORT_EDGE.md), [`../bench/REPORT_ACCURACY.md`](../bench/REPORT_ACCURACY.md)
