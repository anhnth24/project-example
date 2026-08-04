# fileconv-core — roadmap

Authority cho các issue thuộc lõi chuyển đổi dùng chung là
[`issues/README.md`](issues/README.md). Không đưa issue core vào phase catalog của
Markhand Web.

GitHub milestone dự kiến: `Fileconv Core`. Draft chỉ được đồng bộ lên GitHub sau
câu duyệt canonical `Tôi duyệt draft.` và sau khi vượt qua Definition of Ready.

## Phạm vi

`fileconv-core` sở hữu nhận diện định dạng, đọc/giải mã đầu vào và chuyển đổi sang
Markdown dùng chung cho CLI, desktop, MCP và server. Issue trong catalog này không
tự mở rộng sang UI hoặc contract của từng sản phẩm gọi core.

## Issue-level backlog

Tổng: **1 issue** — **0 Ready**, **1 Backlog**, **0 In progress**, **0 Review**,
**0 Done**.

| Issue | Outcome | Status |
|---|---|---|
| [CORE-01](issues/README.md#core-01--giải-mã-txt-utf-16lebe-sang-markdown) | Giải mã TXT UTF-16LE/BE có BOM sang Markdown-compatible text | Backlog |

## Dependency

```text
CORE-01
```

Không có dependency issue. Security review vẫn bắt buộc vì thay đổi xử lý byte đầu
vào không tin cậy trong converter.
