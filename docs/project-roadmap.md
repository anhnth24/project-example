# Lộ trình dự án

> Trạng thái tính đến 2026-07-11. Backlog rút từ [`../bench/RESEARCH_COMPETITORS.md`](../bench/RESEARCH_COMPETITORS.md)
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
- **Subscription bridge**: Cursor Agent và OpenAI Codex CLI dùng browser login,
  ask/read-only sandbox, timeout và fallback; không đọc token.
- **Neural embeddings tùy chọn**: Ollama/LM Studio/vLLM/OpenAI/Gemini, index
  signature + dimension guard + FTS fallback.
- **Audio no-speech**: lọc theo xác suất segment và marker nhạc/im lặng; tự tìm
  PhoWhisper đã tải về trước model chuẩn.
- **Desktop release foundation**: identity `Markhand`, icon đa nền tảng,
  CI/release matrix và `.deb` Linux đã build/kiểm tra metadata.

## Đang làm / Gần ✋

- Dọn **logic trùng** đếm slide PPTX (CLI shell `python3` vs `probe.rs` native).

## Backlog

### Độ chính xác tiếng Việt
- [ ] **Phục hồi dấu IN HOA đầy đủ**: đã thêm retry PSM 6 khi output sparse/lỗi/
      dính chuỗi IN HOA và chọn output theo quality score; phục hồi bằng VLM vẫn cần corpus thật.
- [ ] **Tách cột trước OCR** cho bảng PDF đa cột (Tesseract đang đọc sai thứ tự cột).
- [ ] **Decode VNI / VPS** đầy đủ (mới có TCVN3).
- [x] **Lọc ảo giác whisper** bằng `no_speech_probability` + marker nhạc/im lặng.

### OCR / Vision tier
- [ ] **Vintern-1B** (VLM on-device) cho tài liệu khó thay/về bên cạnh `ocr_hard` cloud.
- [ ] **PaddleOCR vi** với sắp xếp reading-order (đã có `bench/paddle_test.py` tư liệu, chưa tích hợp).
- [ ] **Chữ viết tay** — cần dữ liệu thật có nhãn (sample hiện là font-render, không phải viết tay thật; accuracy ~47.9% là giới hạn Tesseract).

### Output / cấu trúc
- [ ] **Bảng → HTML** cho ô phức tạp (merge cell, multi-line) thay vì chỉ Markdown table.
- [x] **`ConversionResult.title`** — lấy heading đầu, fallback tên file.

### Desktop / đóng gói
- [ ] **Đóng gói distributable đa OS** — `.deb` Linux đã build; AppImage/MSI/DMG
      có release matrix nhưng Windows/macOS còn cần artifact thật và signing/notarization.
- [x] **Dark mode** LumiBase.
- [x] Đổi `prompt()`/`confirm()` native trong Sidebar thành modal tuỳ chỉnh.
- [x] Thống nhất identity Markhand (`com.anhnth24.markhand`, binary `markhand`).

### Tích hợp / mở rộng
- [ ] **Plugin system** (khoảng trống vs best-in-class).
- [ ] **Benchmark tài liệu hành chính** chuyên ngành (giấy tờ, công văn) để đo độ chính xác thực tế hơn.

## Không ưu tiên (theo YAGNI)
- ASR streaming real-time.
- Đa người dùng / đồng bộ cloud (dự án định vị offline-first, đúng hướng với Nghị định 91/2025 PDPL).

## Tham chiếu chéo
- Động lực thị trường: [`project-overview-pdr.md`](project-overview-pdr.md)
- Điểm yếu chi tiết: [`../bench/REPORT_EDGE.md`](../bench/REPORT_EDGE.md), [`../bench/REPORT_ACCURACY.md`](../bench/REPORT_ACCURACY.md)
