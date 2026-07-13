# Tổng hợp review PR #26 + PR #27 và đánh giá app — 2026-07-11

Nguồn: 4 báo cáo review chi tiết trong cùng thư mục:

- `code-review-260711-2220-pr26-pdf-ocr-report.md`
- `code-review-260711-2220-pr27-core-rust-report.md`
- `code-review-260711-2220-pr27-tauri-backend-report.md`
- `code-review-260711-2220-pr27-frontend-ci-report.md`

Xác minh độc lập bởi controller: `cargo test --workspace` pass trên Windows local
(100 core + 43 desktop + 3 CLI = 146, 0 fail). Frontend `pnpm test` chưa chạy được
local (thiếu node_modules); PR ghi nhận 37 test TS pass trên CI.

## Kết luận chung

Không có lỗi Critical. Chất lượng 2 PR tốt: trust boundary (Tauri command, LLM CLI,
OCR subprocess) viết phòng thủ đúng bài; test thật, encode intent. Tồn đọng 3 High
cần xử lý sớm và ~8 Medium.

## Findings High (đã verify)

| # | PR | Vấn đề | Vị trí | Kịch bản |
|---|----|--------|--------|----------|
| H1 | 26 | PDFium gọi cross-thread không mutex; PR26 mở rộng từ "chỉ khi OCR" → mọi convert PDF. libpdfium single-thread by contract; pdfium-render 0.9.2 `thread_safe` chỉ share binding, KHÔNG lock FPDF call | `crates/core/src/conv/pdf.rs:364-372,503-532,849` | Watch-convert + convert tay chạy đồng thời qua spawn_blocking → UB/crash. Fix: mutex toàn tiến trình quanh call PDFium |
| H2 | 27 | CSV formula injection trong `09-JIRA-IMPORT.csv`: nhánh sinh Jira CSV bỏ qua guard tiền tố `=/+/-/@` mà `table_to_csv` có | `crates/core/src/intelligence.rs:1459-1469` (guard đúng ở :888) | Dòng tài liệu độc hại khớp story-extraction → formula chạy khi BA/PM mở bằng Excel/Sheets. File này đúng là để import trực tiếp |
| H3 | 27 | Race trong Intelligence Q&A: đổi scope giữa chừng không hủy request cũ → answer của scope A,B render dưới scope C, citation ngoài scope | `app/src/components/IntelligenceView.tsx:167-189,212-222` | Cần request-id/scope guard hoặc AbortController |

## Findings Medium chọn lọc

- PR26: 1 trang không resolve được → cả tài liệu rơi xuống pdf-extract (mất cấu trúc) — `pdf.rs:426,469-471`.
- PR26: `strip_repeated_marginal_lines` có thể xóa nhầm dòng thân bài tiếng Việt chứa chuỗi trùng header — `pdf.rs:774-837`.
- PR26: fan-out re-extraction N lần khi marker/table fail — `pdf.rs:157-168`.
- PR27 core: citation offset lệch trên tài liệu CRLF (Windows) — `intelligence.rs:427-430` + `chunk.rs:44-52`; lỗi im lặng, span sai.
- PR27 core: `redact_pii`/`update_markdown_table` thiếu check `is_char_boundary` trước `replace_range` — `intelligence.rs:719-731,868-880`. **Đã verify khó chạm trong luồng desktop bình thường** (desktop parse lại document ngay trước khi áp: `app/src-tauri/src/intelligence.rs:645,727`) → hạ mức thực tế, vẫn nên fix phòng thủ.
- PR27 tauri: HNSW temp dir đặt tên theo PID → 2 build index song song trong 1 tiến trình đè nhau — `vector_index.rs:64,101`; `hybrid_search` mutate index trên đường đọc — `knowledge.rs:783`; load toàn bộ vector vào RAM mỗi search (~100MB tại cap) — `knowledge.rs:693`; version snapshot phình vô hạn — `intelligence.rs:780`.
- PR27 FE: cache module-level (`cachedHandoff`...) sống xuyên project switch — `IntelligenceView.tsx:75-77`; rebuild index per-file trong queue convert (O(N²) nếu backend không incremental) — `store.ts:565-569`.

## Hạ mức / non-issue (đã verify)

- "Webview ghi abs path tùy ý" (tauri H-1): mọi abs path trong frontend hợp lệ đều
  từ dialog save/open (`IntelligenceView.tsx:364-366`, `store.ts:434`); CSP
  `script-src 'self'` + rehype-sanitize + PPTX render bằng JSX escape → chỉ là
  defense-in-depth, khuyến nghị hardening (whitelist extension) chứ không phải lỗ hổng đứng một mình.
- API key: chỉ persist qua Rust backend, không localStorage, không log, env-strip khi spawn CLI.
- SQL: parameterized toàn bộ; FTS5 injection đóng bằng tokenize+quote.
- Path traversal: `resolve_within` chặn `..`/absolute/symlink per-component, có test.
- `Pdfium::default()` fallback an toàn (chỉ chạm sau AlreadyInitialized).

## Đánh giá app hiện tại

**Điểm mạnh**

- Lõi convert 9 định dạng ổn định: corpus 90 file internet pass 90/90, PDF 6.02 ms/trang (release), hash-pinned lock file.
- PR26 fix đúng root cause (false-OCR 3→0 trang, 17.54s→0.4s trong Tauri dev) với benchmark tái lập được.
- Intelligence/Handoff suite chạy deterministic offline: pack 15 artifact trong 0.1s, citation coverage 100%.
- 146 test Rust pass local Windows; CI + release workflow đã có (Linux .deb build được).
- Bảo mật tổng thể tốt cho app desktop local-first.

**Rủi ro chính theo thứ tự**

1. H1 PDFium concurrency — duy nhất có thể crash/UB production, nên fix trước khi phát hành desktop cho người dùng thật.
2. H2 CSV injection — một guard đã có sẵn, chỉ cần áp vào nhánh Jira CSV.
3. H3 race Q&A scope — sai lệch niềm tin vào citation, trái với selling point "câu trả lời có căn cứ".
4. CRLF citation drift — đáng chú ý vì user mục tiêu dùng Windows.

**Còn thiếu so với mục tiêu phát hành**: Win/Mac signing cần credential của owner
(workflow đã sẵn, đúng như PR mô tả); `pnpm install` local đang bị chặn nên chưa
tự verify frontend test/build; PhoWhisper license chưa rõ trước khi phân phối.

## Câu hỏi mở

1. Backend `rebuildKnowledgeIndex` có incremental theo file không? (quyết định mức nghiêm trọng của finding store.ts O(N²)).
2. Entitlements macOS JIT/unsigned-exec-memory có dep nào thật sự cần không, hay thu hẹp được?
