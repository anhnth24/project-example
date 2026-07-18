# ADR 0001: Ranh giới dependency của Markhand Web

- Status: Accepted
- Date: 2026-07-17
- Owners: `@anhnth24`
- Approver: repository owner hoặc người được CODEOWNERS chỉ định
- Supersedes: N/A

## Context

Markhand Desktop đã chạy local-first trên `fileconv-core`. Markhand Web cần dùng lại
conversion và RAG logic mà không để desktop, HTTP framework hoặc storage quyết định
kiến trúc lõi. Một dependency ngược ở giai đoạn scaffold sẽ biến web thành fork của
desktop hoặc khiến repository/service không enforce được tenant context.

## Decision

Dependency direction là:

```text
fileconv-core
    ↑
fileconv-knowledge
    ↑                 ↑
desktop adapters   server services
                        ↑
                 routes / workers

web → generated API client → server
```

| Boundary | Được phép | Cấm |
|---|---|---|
| `fileconv-core` | conversion, chunk, deterministic intelligence, optional LLM clients | Tauri, axum, database, Qdrant, MinIO, browser APIs |
| `fileconv-knowledge` | pure retrieval/ranking/grounding/citation types; dependency vào core | Tauri, axum, direct PostgreSQL/Qdrant/MinIO adapters mặc định |
| desktop adapters | gọi core/knowledge qua adapter riêng | bị server hoặc web import ngược |
| server routes/workers | route → service → repository/adapter | route truy cập DB/storage trực tiếp |
| repository | transaction/query với `OrgContext` rõ ràng | business read/write thiếu tenant context |
| `web/` | browser React/TypeScript và generated API client | `@tauri-apps/*`, IPC Tauri, filesystem desktop |

`vendor/markitdown-rs` chỉ là tư liệu MIT, đã bị exclude khỏi workspace và không được
trở thành dependency/path dependency.

## Consequences

- Tách `fileconv-knowledge` ở Phase 1A, không copy RAG desktop vào server.
- SQLite/HNSW desktop adapters chỉ tồn tại sau opt-in features; default/server tree
  không kéo các dependency này.
- Server cần interface adapter cho PostgreSQL/Qdrant/MinIO; generic trait chỉ được tạo
  khi có ít nhất hai consumer thực tế.
- Mọi business repository API ở web server bắt buộc truyền `OrgContext`; missing scope
  là fail-closed.
- OpenAPI là contract giữa web/server. Web không phụ thuộc Rust/Tauri type hay command.
- `scripts/check-architecture-boundaries.py` là baseline CI; exception phải đi kèm ADR
  mới, expiry và test regression.

## Alternatives considered

1. Để server gọi trực tiếp desktop SQLite/HNSW: loại vì không hỗ trợ multi-org,
   transaction và ACL web.
2. Cho route dùng query/storage trực tiếp: loại vì tenant/audit policy không còn điểm
   enforce thống nhất.
3. Shared “platform” crate chứa mọi adapter: loại vì abstraction trước consumer làm
   dependency graph mơ hồ.

## Verification

- `python3 scripts/check-architecture-boundaries.py`
- `python3 scripts/check-architecture-boundaries.py --self-test`
- `make check-knowledge-extraction`
- Review theo `docs/conventions/dependencies.md` và CODEOWNERS.
