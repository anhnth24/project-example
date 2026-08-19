# Lộ trình dự án

> Trạng thái tính đến 2026-08-18. Backlog desktop rút từ
> [`../bench/RESEARCH_COMPETITORS.md`](../bench/RESEARCH_COMPETITORS.md) và điểm yếu đã biết
> trong các `REPORT*.md`. Tiến độ Markhand Web: [`../plans/markhand-web/README.md`](../plans/markhand-web/README.md).

## Đã hoàn thành ✅

- **Lõi convert** (`fileconv-core`): pdf/docx/pptx/xlsx/csv/html/text + ảnh OCR + audio → Markdown.
- **PDF 3-tier**: pdf-inspector (cấu trúc, đa cột, cờ `needs_ocr`) → pdfium-render → pdf-extract fallback.
- **PDFium thread-safety lock**: serialized `PDFIUM_CALL` để tránh UB concurrent PDF conversion.
- **Tiền xử lý ảnh OCR**: grayscale → upscale → unsharpen → normalize (in OCR 98.5→99.5%, low-res 81→99%).
- **NFC bắt buộc** trên mọi output (sửa tài liệu NFD từ macOS/PDF cũ).
- **Decode TCVN3/VNI-Windows/VPS** với map VietUnicode trong CSV/text; opt-in
  `Tcvn3CaseHint` khôi phục hoa TCVN3/ABC all-capital H-font khi có font/run
  metadata (không đoán từ TXT/CSV).
- **RAG chunking** theo heading-path (`chunk.rs`).
- **CLI bench**: `one` / `speed` / `accuracy` / `audio` với CER/WER Levenshtein.
- **MCP server** (`fileconv-mcp`): 8 tool (4 deterministic + 4 LLM, gồm `ocr_hard` vision).
- **Desktop app "Markhand"** (Tauri 2 + React): kéo-thả, soạn thảo, xem trước nguồn, cài đặt OCR/audio.
- **Auto-update via GitHub Releases**: v0.1.0 + minisign signatures, check on init, install từ Settings.
- **CI desktop installers**: Linux/Windows/macOS matrix trên master push + tags `markhand-vX.Y.Z`, version-check từ tauri.conf.json.
- **Windows console flash fix**: `proc::background_command()` set CREATE_NO_WINDOW khi spawn tesseract/LLM CLI.
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
  signature + dimension guard + persistent HNSW + exact/FTS fallback.
- **Audio no-speech**: lọc theo xác suất segment và marker nhạc/im lặng; tự tìm
  PhoWhisper đã tải về trước model chuẩn.
- **Desktop release foundation**: identity `Markhand`, icon đa nền tảng,
  CI/release matrix và `.deb` Linux đã build/kiểm tra metadata.
- **PPTX preview**: parser OOXML Rust + SVG React cho text/ảnh/shape, navigation
  bàn phím; chart/SmartArt có placeholder.
- **Live watch folders**: notify recursive, debounce, chống loop DATA và tự đẩy
  file mới vào queue frontend.
- **Bảng merge/multiline**: XLSX/XLS/DOCX fallback HTML rowspan/colspan và render
  qua sanitizer.
- **Đếm slide PPTX hợp nhất native**: CLI bench dùng `fileconv_core::probe`
  (bỏ nhánh shell `python3` trùng lặp; hai logic đã đối chiếu cho cùng kết quả).

## Đang làm / Gần ✋

### Markhand Web (on-prem RAG platform)

Phase F, 0 và 1A **đã đóng** (32/32 issue). Phase 1B **24/24 Done — gate đóng**
(R06 hanging soak pass 2026-07-31). R02–R05 Done với evidence CI rust-integration
trên `b5cc92c` (run 30603158015). O-chain release (O01–O05) và soak qualification đã
pass trên live infrastructure (2026-07-26). Phase 1C **10/13 Done** (1C-04/07/09/10/11
đóng trên CI `6833f57`, run
[30678318560](https://github.com/anhnth24/project-example/actions/runs/30678318560);
1C-01/02/03/05/06 đã Done trước đó), **2 In progress** (1C-08 CI half / deployed → PR 5;
1C-12 denial suite), **1 Backlog** (1C-13). `AR-1C-AUDIT-RETENTION` giữ POC-only —
không claim retention. Exit gate denial/security còn mở. Phase 2 **13/19 Done**
trên mock/CI (#311–#318, #327, #332); còn P2-10, P2-15, P2-16 và ba issue mở rộng
P2-17…19. Phase 3–4 chưa activate (Backlog). Catalog aggregate: **79 Done**,
**8 In progress**, **0 Review**, **29 Backlog**.

**Workstream ngoài catalog — nâng cấp chất lượng Q&A (2026-08-17/18,
[`../plans/260817-qa-quality-upgrade.md`](../plans/260817-qa-quality-upgrade.md)):**
đã xong P0.0 (retry embedding transient), P0.0b (migration 0037 split file-token
FTS — "công văn 1502" match được `1502/CV-CNTT`), P0.2 (auto-attach citation
propose-then-validate); eval đa định dạng v2 85.7 → **92.9**, eval version giữ
89.1. Cùng đợt (chưa có issue): convert worker Windows same-host qua subprocess
(`workers/converter_subprocess.rs`), chuẩn hoá Markdown sau OCR
(`services/ocr_normalize.rs`), vòng UX web (full-screen, scroll lịch sử chat,
dịch warning tiếng Việt) và chitchat routing (đã trên master). Chín hạng mục còn
lại của plan được track thành **Phase 2Q** trong catalog
([`../plans/markhand-web/backlog/phase-2q/issues/README.md`](../plans/markhand-web/backlog/phase-2q/issues/README.md)).

Dashboard: [`../plans/markhand-web/roadmap.html`](../plans/markhand-web/roadmap.html) ·
Issue catalog: [`../plans/markhand-web/backlog/`](../plans/markhand-web/backlog/)

## Backlog

### Độ chính xác tiếng Việt
- [ ] **Phục hồi dấu IN HOA đầy đủ**: cơ chế retry PSM 6 thời Tesseract đã retired
      cùng engine (ADR 0016); cần đo lại IN HOA trên vision OCR với corpus thật.
- [x] **Tách cột trước OCR** bằng vertical projection, OCR từng cột và score fallback.
- [x] **Decode VNI / VPS** đầy đủ từ bảng tham chiếu VietUnicode.
- [x] **Lọc ảo giác whisper** bằng `no_speech_probability` + marker nhạc/im lặng.

### OCR / Vision tier
- [x] **Local VLM/Vintern integration** qua endpoint vision OpenAI-compatible,
      model do người dùng chọn; không bundle weight/license chưa rõ.
- [x] ~~PaddleOCR vi opt-in~~ retired cùng Tesseract (2026-08-10, ADR 0016) —
      OCR chuyển hoàn toàn sang vision-LLM (OpenRouter mặc định, `FILECONV_OCR_*`);
      server dùng deferred OCR qua sandbox artifact.
- [ ] **Đo hậu kiểm CER/WER** vision OCR trên corpus scan vi (so baseline
      Tesseract cũ trong `bench/REPORT_ACCURACY.md`), gồm cả chữ viết tay.

### Output / cấu trúc
- [x] **Bảng → HTML** cho merge cell/multiline, sanitize khi preview.
- [x] **`ConversionResult.title`** — lấy heading đầu, fallback tên file.

### Desktop / đóng gói
- [x] **CI release matrix Linux/Windows/macOS** — `.deb` Linux đã build thực tế; 
      MSI/DMG artifact trong matrix nhưng cần signing/notarization thực.
- [x] **Auto-update framework** — GitHub Releases endpoint, minisign signature, version check từ tauri.conf.json.
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
