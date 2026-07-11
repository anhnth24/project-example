# Báo cáo Markhand Intelligence / Handoff

Ngày đo: 2026-07-10. Corpus thử: PDF FPT CASAN 45 trang đã convert, chạy Tauri
desktop thật trong Xvfb 1440×900, chế độ deterministic offline.

## Kết quả

- Thời gian sinh pack qua desktop: **0,103 giây**.
- Citation coverage: **100%**.
- Validation: **đạt**.
- 700 mục có ID/trạng thái/trích dẫn.
- 348 cảnh báo `needs_elaboration` để BA/PM rà soát.
- 15 file được ghi dưới `DATA/.markhand/handoff/<pack-id>/`.

Artifacts:

1. README
2. BRD
3. PRD
4. User stories
5. Acceptance criteria
6. Glossary
7. Test cases
8. Traceability matrix
9. Assumptions/open questions
10. Jira CSV
11. GitHub issue drafts
12. Confluence Markdown
13. Obsidian MOC
14. Manifest JSON
15. Validation JSON

## Các chức năng đã kiểm tra

- Handoff workspace và chọn corpus.
- BRD/PRD editor + lưu artifact.
- Quality report (PDF CASAN: 100 điểm, 8 bảng).
- Search/Q&A có citation.
- Version snapshot/diff/three-way merge.
- Markdown table editor + CSV export.
- PII scan và xuất bản đã che.
- Watch-rule persistence + manual polling/import.
- Knowledge Pack ZIP.
- Optional LLM/vision hooks dùng `FILECONV_LLM_*`; không có key vẫn giữ bản tất định.

## Lưu ý chất lượng

CASAN là tài liệu phương pháp luận, không phải đặc tả sản phẩm hoàn chỉnh. Vì vậy
baseline tất định nhận nhiều câu có từ “phải/cần” và sinh skeleton user story/AC/test
case ở trạng thái `needs_elaboration`. Đây là hành vi có chủ đích: hệ thống không tự
bịa persona, SLA hay luồng còn thiếu.

Với corpus BRD/workshop/Excel traceability thật, BA/PM nên:

1. gán đúng tập nguồn;
2. xem cảnh báo và câu hỏi mở;
3. chỉnh/duyệt BRD/PRD;
4. chỉ bật LLM nếu chấp nhận gửi các đoạn citation tới provider.

## Regression coverage

Sau review, test suite được mở rộng từ các happy-path cơ bản thành **187 test**
chạy tự động:

| Lớp | Số test | Phạm vi |
|---|---:|---|
| `fileconv-core` | 102 | convert/OCR/audio/PPTX/legacy + intelligence + LLM protocols |
| Tauri desktop | 45 | path jail, watch service, HNSW, settings, persistent RAG |
| React/TypeScript | 37 | blocks, project scope, provider/subscription/embedding helpers |
| CLI metrics | 3 | CER/WER |

Nhóm intelligence bao phủ:

- corpus IDs, CRLF/UTF-8 offsets và page anchors;
- search ranking/limit/accent folding và Q&A không có kết quả;
- SQLite FTS persistence/incremental replacement, scoped hybrid ranking, local
  vectors, query-syntax safety và fallback khi thiếu/hỏng LLM provider;
- quality score cho short/OCR/repeated/encoding;
- PII positive/negative/context/repeated/redaction bounds;
- Markdown table parse/escape/update, schema typing và CSV formula injection;
- diff added/removed/unchanged, merge clean/conflict;
- watch glob và target state;
- BR/FR/US/AC/assumption/question extraction;
- stable IDs, đủ artifact, duplicate/missing/weak citations, empty requirements;
- ZIP atomic replacement và archive completeness;
- Tauri `.markhand`/sidecar symlink rejection, corrupt snapshot propagation,
  version ID traversal và pack round-trip.
- local OpenAI-compatible không cần key, cloud Bearer auth, URL `/v1`, provider
  aliases/presets, settings migration và API key không persist.
- Cursor/Codex official CLI parsing, ask/read-only args, stdin transport, timeout
  kill và subscription status.
- neural embedding batch/normalization, persistent model signature/dimensions,
  mixed-dimension rejection, query vector và whole-scope local fallback.
- VNI/VPS maps/detection, column OCR, HTML rowspan/colspan, PPTX preview shapes,
  notify loop safety và HNSW dump/reload.
- project discovery/legacy migration, Unicode slug, nested folder collection,
  import limits, supported formats, no-overwrite copy và project-scoped tree.

## Desktop release verification

- Identity: `Markhand`, bundle ID `com.anhnth24.markhand`, binary `markhand`.
- Sinh đủ icon Linux/macOS/Windows/mobile từ source SVG.
- `pnpm tauri build --bundles deb` thành công trên Linux.
- Artifact: `Markhand_0.1.0_amd64.deb`, khoảng **16.7 MB**.
- Debian metadata: package `markhand`, section `utils`; depends
  `libwebkit2gtk-4.1-0`, `libgtk-3-0`; recommends Tesseract + tiếng Việt.
- CI test và release matrix đã thêm cho Linux/Windows/macOS.
