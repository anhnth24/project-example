# Markhand Web RAG Architecture Brainstorm

**Date**: 2026-07-13 14:00
**Severity**: Medium
**Component**: Markhand Web, fileconv-core reuse, crates refactor
**Status**: Resolved

## What Happened

Session brainstorm toàn vẹn cho Markhand Web — web quản lý tài liệu + RAG multi-org xây trên fileconv-core. User hôm nay chốt được kiến trúc hệ thống từ backend tới frontend, phasing plan 5 giai đoạn, và danh sách 8 security/reliability findings từ Codex review cần integrate vào design.

## The Brutal Truth

80% logic RAG mà cần cho web đã tồn tại trong desktop app (hybrid FTS5+vector, citation tracking, HNSW indexing) — nhưng gắn chặt vào Tauri, SQLite, sync-blocking I/O. Phát hiện này là tốt: không cần reinvent wheel. Cục gian là cần tách crates/knowledge + refactor I/O để dùng chung được. Cảm giác bối rối khi nhận ra cần _refactor_ thay vì _build_ từ đầu, nhưng strategy "extract library" hiệu quả hơn "rewrite".

## Technical Details

**Architecture chốt:**
- Backend: Rust (axum). PG system-of-record, Qdrant vector, MinIO file gốc.
- Embedding: interim GLM cloud (`embedding-3`) cho POC/DEMO; target vLLM GPU
  (bge-m3/e5) — ADR 0004. Chat: GLM cloud (có phép nhận nội dung tài liệu).
- Auth: RBAC mức 2 (role per-org + ACL collection). Rate limit: tower_governor + quota reserve/finalize atomic.
- Desktop app tạm giữ SQLite FTS5 hybrid (không thay đổi desktop flow).
- Frontend: React+Vite SPA, on-prem. Scale: mỗi org ~vài trăm GB, tổng ~vài TB.

**Codex findings 8 cái hấp thụ:**
- Document state machine + idempotency + tombstone + reconciliation (consistency PG/Qdrant/MinIO)
- Tenant-scoped repository + denial tests
- Upload hardening: MIME sniff, zip-bomb check, sandbox, timeout
- Quota atomic
- BLAKE3 thay DefaultHasher
- Embedding queue backpressure
- Phasing lại (không generic hóa storage sớm)

## Phasing Plan

**Phase 0**: Spike benchmark Qdrant/PG FTS + eval embedding golden-set tiếng Việt (gate).
**1A**: Tách crates/knowledge, giữ hành vi desktop.
**1B**: Vertical slice single-org = POC hiện tại (1 đơn vị, vài account test).
**1C**: Multi-org đầy đủ.
**2**: Web SPA.
**3**: Intelligence (LLM integration).
**4**: Hardening + SSO/OIDC + help page.

## Why This Decision

Không tái xây RAG từ đầu: design đã proven ở desktop, chỉ cần decouple khỏi Tauri+SQLite. Phasing theo "vertical slice" (end-to-end nhỏ trước) tránh scope creep. Codex findings + atomic quota + encryption là defensive — tối thiểu hóa risk multi-tenant.

## Lessons Learned

- **Architecture review early**: 8 findings từ Codex review giúp tránh refactor lại sau. Hấp thụ vào phase 0 ít hơn phase 3.
- **State machine thinking**: Document state + idempotency chặn race condition. Rõ ràng hơn try-catch.
- **Tenant boundary cứng**: Deny-by-default, ACL explicit trên mọi query (không trust session ctx).

## Next Steps

1. **Provisioning** (user): xác nhận GPU server + PG/Qdrant/MinIO infrastructure.
2. **Planning** (/ck:plan): chi tiết phase 0 spike benchmark (golden-set, Qdrant tuning, PG FTS vs Qdrant recall).
3. **Crates refactor** (phase 1A): tách knowledge.rs + vector_index.rs ra crates/knowledge.
4. **Test golden-set tiếng Việt**: embedding + retrieval recall trên 500-1K documents.

---

**Decision locked**. Backend architecture approved. Ready to move to detailed planning once infra provisioned.
