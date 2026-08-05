# Thiết kế cập nhật tài liệu codebase

## Mục tiêu

Cập nhật các tài liệu sống mô tả codebase để phản ánh đúng `master` hiện tại, giúp
contributor tìm đúng module, hiểu đúng các surface sản phẩm và chạy đúng luồng local.
Mọi khẳng định về trạng thái hoặc hành vi phải có bằng chứng từ manifest, source,
OpenAPI, Makefile, script hoặc test hiện hành.

## Phạm vi

Các tài liệu chính:

- `docs/codebase-summary.md`
- `docs/system-architecture.md`
- `docs/project-overview-pdr.md`
- `docs/code-standards.md`
- `docs/runbooks/local-development.md`

Runbook sống khác chỉ được sửa nếu nội dung liên kết trực tiếp tới các luồng trên và
có bằng chứng rõ ràng rằng câu lệnh hoặc trạng thái đã sai.

Ngoài phạm vi:

- roadmap và issue catalog;
- ADR, journal, design spec và implementation plan đã đóng vai trò lịch sử;
- báo cáo benchmark và evidence đã chốt;
- tài liệu ngoài `docs/`;
- PostgreSQL ERD, vì đây là một outcome riêng cần regenerate và validate schema.

## Nguồn sự thật

Ưu tiên đối chiếu theo thứ tự:

1. Workspace/package manifests và lockfiles.
2. Source đăng ký command, route, tool, feature và adapter.
3. OpenAPI, migrations và script vận hành.
4. Makefile, CI workflow và test hiện hành.
5. Issue catalog chỉ để giải thích maturity; không dùng thay bằng chứng code.

Không suy trạng thái triển khai chỉ từ roadmap hoặc tài liệu cũ.

## Thay đổi thiết kế

### Bản đồ codebase

`codebase-summary.md` sẽ mô tả đầy đủ `fileconv-core`, CLI, MCP,
`fileconv-knowledge`, `fileconv-server`, desktop Tauri, Web SPA và deployment
surface. Bảng LOC dễ lỗi thời sẽ bị bỏ hoặc thay bằng mô tả định tính. Các bảng module
sẽ dùng đường dẫn và trách nhiệm hiện tại, gồm PDF module phân tách, detailed
conversion, knowledge adapters, server routes/workers và các vùng chính của Web SPA.

### Kiến trúc hệ thống

`system-architecture.md` sẽ mở rộng từ mô hình ba giao diện sang hai nhánh:

- converter surfaces: CLI, desktop và MCP dùng `fileconv-core`;
- Markhand Web: browser SPA → HTTP/SSE API → service/repository/storage/workers,
  dùng contracts từ `fileconv-knowledge` và converter qua worker.

Các inventory cụ thể sẽ được đồng bộ với code: format `Text`, detailed conversion,
title derivation, CLI commands, MCP tools, Tauri IPC và capability. Số đếm dễ drift
chỉ giữ khi code có một registry rõ ràng; nếu không sẽ dùng mô tả nhóm.

### PDR

`project-overview-pdr.md` sẽ bổ sung Markhand Web như một product track riêng thay vì
ngụ ý dự án chỉ có desktop/CLI/MCP. Các mâu thuẫn về installer và dark theme sẽ được
sửa; giới hạn còn lại được diễn đạt chính xác là signing/notarization và evidence phát
hành phù hợp, không phải chưa có bundle.

### Quy ước code

`code-standards.md` sẽ cập nhật đường dẫn PDF module, cơ chế temp file OCR, feature
`audio`, pin digest ở workspace và xóa cạm bẫy PPTX/Python đã được giải quyết. Các pin
có chủ đích, cache bounded/thread-local, GNU linking và converter boundary phải được
giữ nguyên.

### Local development

`runbooks/local-development.md` sẽ phản ánh API upload/jobs, search/ask và Web SPA đã
có trên `master`. E2E flow sẽ mô tả upload accepted enqueue convert, các worker cần
chạy để hoàn tất convert/index/embedding và cách quan sát job. Phần “route chưa có”
sẽ được thay bằng ví dụ kiểm tra route hiện hành. Các bảng port, image pin, auth seed
và embedding profile đang đúng sẽ được giữ nguyên.

## Nguyên tắc chống drift

- Tránh số LOC và số lượng file.
- Với command/tool/route, ưu tiên liệt kê từ registry hoặc OpenAPI.
- Tách rõ “đã có trong code”, “cần runtime ngoài” và “đã có evidence production”.
- Không đổi status roadmap chỉ vì code đã tồn tại.
- Giữ liên kết tới tài liệu chuyên sâu thay vì lặp cấu hình dài ở nhiều nơi.

## Kiểm chứng

- Đối chiếu lại mọi đường dẫn và symbol được thêm.
- Tìm các claim cũ đã xác định: `18 IPC`, `56 command`, `8 tool`,
  `title: None`, PPTX qua Python, route search/ask chưa có và upload chưa enqueue.
- Chạy Markdown/link checks hiện có nếu repository cung cấp.
- Chạy quality gate tĩnh liên quan tới docs và roadmap; không sửa generated roadmap.
- Xem diff cuối để bảo đảm không thay đổi tài liệu lịch sử hoặc mở rộng sang outcome
  ERD.

## Tiêu chí hoàn thành

- Năm tài liệu trong phạm vi không còn các discrepancy đã audit.
- Contributor có thể lần từ tài liệu tới đúng crate/module/route hiện tại.
- Luồng local upload → worker → retrieval được mô tả đúng maturity và dependency.
- Không có claim benchmark, production readiness hoặc release evidence mới khi chưa có
  bằng chứng.
- Các kiểm tra tài liệu liên quan vượt qua.
