# Dependency rules

Quy ước này hiện thực ADR [`0001-web-boundaries.md`](../adr/0001-web-boundaries.md).
Mọi PR tạo crate, package, route, worker hay repository mới phải tuân theo.

## Allowed direction

```text
core ← knowledge ← {desktop adapter, server service} ← {route, worker}
web → generated API client → server
```

- `core` độc lập framework, storage và UI.
- `knowledge` pure mặc định; adapter desktop/server là consumer ở ngoài.
- Route parse/auth/serialize; service giữ use case; repository/adapter giữ I/O.
- Web chỉ dùng HTTP/SSE contract đã publish; không import Tauri.

## Mandatory rules

1. Không thêm `tauri`, `axum`, `sqlx`, `rusqlite`, Qdrant, MinIO/S3 hoặc browser API
   vào `fileconv-core`.
2. Không thêm Tauri/axum/direct storage dependency vào `fileconv-knowledge` mặc định.
   Feature adapter phải được ADR phê duyệt.
3. Route/handler không gọi database, object storage hay vector client trực tiếp.
4. Business repository method nhận `OrgContext` (hoặc context type kế thừa rõ ràng);
   worker/reconcile cũng không được bypass scope này.
5. Không package/import bất kỳ code nào từ `vendor/markitdown-rs`.
6. `web/` không import `@tauri-apps/*`, `window.__TAURI__`, hay desktop IPC wrapper.
7. Dependency mới vượt boundary cần ADR, owner, justification, expiry và regression
   test trước khi merge.

## Review checklist

- [ ] Dependency đi theo mũi tên; không có reverse dependency desktop/server → adapter.
- [ ] I/O chỉ ở adapter/repository; route không chạm persistent client.
- [ ] Public business operation có tenant context và test deny khi thiếu/sai org.
- [ ] Code generated OpenAPI không sửa tay; web không dùng Tauri.
- [ ] `cargo metadata --no-deps` và boundary check xanh.

## Automated baseline

```bash
python3 scripts/check-architecture-boundaries.py
python3 scripts/check-architecture-boundaries.py --self-test
```

Script là guard baseline, không thay security review. Nó kiểm tra direct Cargo dependency
cấm, workspace không kéo vendor, web imports Tauri và direct I/O trong future route
directories. Khi F-02 tạo skeleton, checks tự activate theo các đường dẫn đó.
