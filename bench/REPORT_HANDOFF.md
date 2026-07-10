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
