# fileconv-core issues

Parent roadmap: [`../README.md`](../README.md)

GitHub milestone dự kiến: `Fileconv Core`

## Dependency

```text
CORE-01
```

Trường `Dependencies/blocks` là authority. Catalog này chỉ sở hữu outcome thuộc
`fileconv-core`; thay đổi UI/server ngoài core phải có issue riêng.

## CORE-01 — Giải mã TXT UTF-16LE/BE sang Markdown

- **Status:** Backlog
- **Objective:** `Converter::convert_path` giải mã chính xác file `.txt` tiếng Việt
  UTF-16LE hoặc UTF-16BE có BOM thành Markdown-compatible Unicode text, không để BOM
  lọt vào output và không làm đổi hành vi UTF-8 hoặc bảng mã tiếng Việt legacy.
- **Implementation plan:** Trong `conv/text.rs`, nhận diện BOM UTF-16LE `FF FE` và
  UTF-16BE `FE FF` trước nhánh UTF-8/legacy hiện có; ghép từng cặp byte thành `u16`
  theo đúng endian rồi decode UTF-16 nghiêm ngặt bằng thư viện chuẩn. Trả
  `ConvertError::Failed` với thông báo không chứa nội dung tài liệu khi payload có
  số byte lẻ hoặc surrogate không hợp lệ. Giữ nguyên strip UTF-8 BOM,
  `viet_legacy::decode_text` cho đầu vào không có BOM UTF-16, và cổng NFC chung sau
  conversion.
- **Files/modules:** Owner `fileconv-core`, reviewer theo `CODEOWNERS`
  (`@anhnth24`); boundary `crates/core/src/conv/text.rs` và test cùng module.
- **Dependencies/blocks:** Không có dependency code hoặc external gate. Draft chờ
  câu duyệt canonical trước khi chuyển `Ready`.
- **Acceptance criteria:** `.txt` UTF-16LE có BOM và `.txt` UTF-16BE có BOM chứa
  tiếng Việt đi qua public `Converter::convert_path` cho nội dung Unicode chính xác,
  `FormatKind::Text`, không có `U+FEFF`, và output cuối ở NFC; UTF-8 có/không BOM,
  TCVN3, VNI và VPS giữ nguyên behavior; payload UTF-16 có số byte lẻ hoặc surrogate
  không hợp lệ trả lỗi xác định, không panic và không trả partial/lossy text.
- **Required tests/evidence:** Unit test deterministic cho UTF-16LE, UTF-16BE,
  UTF-8 regression, legacy regression, odd byte count và unpaired surrogate trong
  `conv::text::tests`; test public route qua `Converter::convert_path`; chạy
  `cargo test -p fileconv-core conv::text::tests`,
  `cargo test -p fileconv-core viet_legacy::tests`, `cargo fmt --all -- --check`,
  `cargo metadata --locked --format-version 1 --no-deps` và
  `python3 scripts/check-dependency-policy.py`.
- **Security/migration:** Security trigger đã được đánh giá: implementation PR bắt
  buộc owner/security review vì converter xử lý byte đầu vào không tin cậy. Không
  thêm dependency, subprocess, network/egress, secret, schema, API DTO hoặc
  migration; decode tuyến tính trên buffer mà converter hiện đã đọc toàn bộ. Error
  không được chứa document content; negative tests khóa malformed
  length/surrogate và no-panic behavior.
- **Out of scope:** Suy đoán UTF-16 không BOM; UTF-32; thay đổi CSV hoặc extension
  allowlist; behavior riêng hoặc fixture riêng cho `.log`/`.md`/`.markdown`; suy
  diễn heading/cấu trúc Markdown từ plain text; UI, server upload policy, dependency
  mới và refactor `viet_legacy`.
