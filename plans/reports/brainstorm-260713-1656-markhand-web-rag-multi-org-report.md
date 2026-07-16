# Brainstorm Report — Markhand Web: quản lý tài liệu + RAG multi-org

- Date: 2026-07-13
- Status: Design APPROVED bởi user; đã qua **Codex review** (8 finding) và cập nhật thiết kế + phasing theo review. User đã trả lời 4 câu hỏi mở của review.
- Modes: none (không --html/--wiki)
- Implementation plans: [`../markhand-web/README.md`](../markhand-web/README.md)

## 1. Bài toán

Xây web app quản lý tài liệu trên nền `fileconv-core`: trích xuất (pdf/docx/pptx/xlsx/csv/html/txt + OCR + audio → Markdown), embed, RAG, phân tích ngữ cảnh, hỏi đáp có citation, kèm hướng dẫn sử dụng. Multi-org, multi-user, on-prem, yêu cầu phân quyền chức năng + rate limit.

**Quy mô đã chốt**: mỗi org ~vài trăm GB tài liệu ([Inference] ≈ 10–20M chunk/org); tổng hệ thống nhiều org → vài TB. **Giai đoạn hiện tại: POC 1 đơn vị, vài tài khoản test** — schema thiết kế multi-org từ đầu nhưng triển khai/kiểm thử theo lát cắt dọc single-org trước.

## 2. Hiện trạng codebase (scout)

- Pipeline RAG đã tồn tại trong desktop: `app/src-tauri/src/knowledge.rs` (1.672 dòng, SQLite FTS5 + vector hybrid + citation), `app/src-tauri/src/vector_index.rs` (HNSW persistent), `app/src-tauri/src/intelligence.rs` (1.168 dòng: quality, PII, BA/PM handoff). Tổng ~5.200 dòng gắn chặt Tauri **và** SQLite/filesystem root/sync-blocking flow (`knowledge.rs:178`, `:1027`) — refactor khó hơn mức "bỏ tauri::State".
- `crates/core`: `chunk.rs` (chunk heading-path), `llm.rs` (chat/vision/embedding đa provider OpenAI-compatible — GLM/vLLM dùng được ngay; lưu ý embed hiện blocking HTTP, batch 64, timeout 60s — `llm.rs:801`), `intelligence.rs` thuần.
- Chưa có web server crate. MCP server (`crates/mcp`) là convert-tool, không phải document store.
- Frontend React+Vite desktop có component tái dùng: `SafeMarkdown`, pattern `LibraryView`/`IntelligenceView`, `ui.tsx`.
- Core suy định dạng theo extension, không sniff magic-byte (`docs/system-architecture.md`) — upload web cần lớp bảo vệ riêng.
- Desktop dùng `DefaultHasher` cho content hash (`knowledge.rs:142`) — không ổn định xuyên version/process, không dùng cho server.

Kết luận: ~80% logic RAG đã có. Bài toán thực = **tách lớp intelligence khỏi Tauri thành crate dùng chung + bọc axum API + web UI + multi-tenancy**.

## 3. Quyết định đã chốt (user)

| Hạng mục | Quyết định |
|---|---|
| User model | Multi-org, multi-user, DB riêng (PG). **POC trước: 1 org, vài tài khoản test** |
| Kiến trúc backend | Tách crate dùng chung + axum API |
| Storage vector | **B: PG + Qdrant** (PG system-of-record; Qdrant vector quantized, disk-backed HNSW) |
| Embedding | **GPU nội bộ + model local qua vLLM** (bge-m3/multilingual-e5); GLM cho chat |
| GLM & dữ liệu | User xác nhận **chấp nhận được** việc GLM nhận prompt chứa nội dung tài liệu (API cloud dùng được; self-host vẫn là lựa chọn nếu chính sách đổi) |
| Auth | JWT tự build cho POC; **SSO/OIDC sẽ có trong tương lai** → auth layer tách riêng, pluggable OIDC |
| RBAC | **Mức 2**: role per-org (owner/admin/editor/viewer) + ACL collection; schema role→permissions dạng bảng để nâng custom-role sau |
| Rate limit | **2 tầng**: request (tower_governor) + quota tài nguyên (LLM token, upload GB, concurrent jobs) |
| Deploy | On-prem nội bộ |
| Frontend | React + Vite SPA mới trong repo, serve static từ axum |
| SLA | User yêu cầu quan tâm **cả 3**: latency, throughput, recovery. Số cụ thể chưa chốt — dùng draft target ở mục 5, chốt lại ở planning |
| Scope tổng | Upload→convert→index→Q&A citation; thư viện tài liệu; intelligence; help page (qua các phase) |

## 4. Phương án đã đánh giá

### Storage vector (tổng vài TB; per-org ~vài trăm GB ≈ 10–20M chunk)
- **A. pgvector partition per-org**: per-org 10–20M chunk là mức biên của pgvector; tổng nhiều org cộng dồn thì rủi ro. Loại.
- **B. PG + Qdrant** ✅: chịu trăm triệu vector, quantization giảm RAM, multitenant qua payload filter. +1 container nhẹ.
- **C. OpenSearch BM25+kNN**: keyword tốt nhất nhưng vận hành JVM cluster nặng nhất on-prem. Loại.

### Kiến trúc tái dùng
- **Tách crate chung** ✅ — một nguồn sự thật, desktop + web cùng dùng. Codex lưu ý: **không generic hóa storage quá sớm** — extract phần thuần (chunk/rank/citation) trước, adapter PG/Qdrant viết theo nhu cầu server, không thiết kế trait tổng quát ngay.
- Copy logic sang web riêng — 2 bản RAG song song. Loại.
- Web qua MCP — không hợp web UI (upload lớn, SSE, session). Loại.

### Embedding
- **Local GPU vLLM** ✅ — API cloud bất khả thi cho ingest 10M+ chunk. Hash 256-d local chỉ là fallback chất lượng thấp.

### RBAC: mức 1 / **mức 2** ✅ / mức 3 (custom role — schema sẵn đường nâng).

### Rate limit: chỉ req/s / **2 tầng** ✅ / full billing metering.

## 5. Thiết kế đã duyệt (đã cập nhật theo Codex review)

### Workspace
- `crates/core` — giữ nguyên.
- `crates/knowledge` (mới) — tách từ `app/src-tauri/src/{knowledge,vector_index,intelligence}.rs`: **extract tối thiểu theo use-case** — logic thuần chunk→embed→rank→citation trước; desktop giữ SQLite/HNSW và hành vi không đổi (test chốt trước khi tách).
- `crates/server` (mới) — axum: JWT auth (module tách riêng, pluggable OIDC sau), RBAC middleware, tower_governor + quota, REST + SSE, worker pool, adapter PG/Qdrant/MinIO.
- `web/` (mới) — React+Vite SPA; thay `ipc.ts` bằng HTTP client; tái dùng SafeMarkdown/pattern view.

### Hạ tầng on-prem
PostgreSQL (metadata/FTS/RBAC/jobs/quota/audit) + Qdrant (vector) + MinIO (file gốc) + vLLM GPU (embedding) + GLM (chat — API cloud được phép; giữ option self-host).

### PG schema (mọi bảng nghiệp vụ có org_id)
- Tenancy: `orgs`, `users`, `org_memberships(role)`, `refresh_tokens`
- RBAC: `roles(org_id nullable)`, `role_permissions`; permission string trong code: `doc.upload|doc.delete|qa.query|member.manage|settings.manage|intel.use|audit.view|export.run`…
- Docs: `collections(visibility: private|org|groups)`, `collection_access`, `documents(status, content_hash BLAKE3, minio_key)`, `document_versions`, `chunks(heading_path, text, tsv)` partition theo org_id. **Content hash dùng BLAKE3/SHA-256** (ổn định xuyên process — không dùng DefaultHasher như desktop).
- Jobs: `jobs(type,status,attempts,locked_by,checkpoint,idempotency_key)` — claim `FOR UPDATE SKIP LOCKED`, resumable
- Quota: `org_quotas`, `usage_counters`, `quota_reservations` (xem Rate limit)
- `audit_log`

### Document state machine + consistency PG/Qdrant/MinIO (finding #2)
Trạng thái tài liệu: `uploaded → converting → converted → indexing → indexed | failed`. Nguyên tắc:
- PG là nguồn sự thật; mọi bước ghi Qdrant/MinIO đều idempotent (idempotency key theo document version + batch).
- **Delete = tombstone**: đánh dấu trong PG trước → job xóa vector Qdrant + object MinIO; citation fetch luôn check trạng thái PG nên tài liệu đã xóa/thu hồi quyền không bao giờ trả ra.
- **Reconciliation job** định kỳ: quét lệch PG↔Qdrant↔MinIO (orphan file, stale vector) và sửa.

### Qdrant
1 collection chung; payload `{org_id, collection_id, document_id, chunk_id}` + payload index org/collection; scalar quantization; text lấy từ PG theo chunk_id. **Phase 0 phải benchmark**: payload filter latency, delete/update payload, snapshot/restore, RAM sau quantization với phân bố org thật.

### Ingest pipeline + hardening upload (finding #4 — làm ngay ở vertical slice, KHÔNG để phase cuối)
1. Upload multipart: **MIME sniff magic-byte đối chiếu extension, giới hạn size/page/duration, chống zip-bomb, quarantine bucket** → check quota (reserve) → MinIO → `documents(uploaded)` + job `convert`
2. Worker convert: chạy fileconv-core với **timeout + kill**, process cách ly (sandbox worker); PDF 3-tier/OCR/audio như hiện tại → Markdown → job `index`
3. `index`: chunk heading-path → **embedding queue riêng** (batch scheduler, backpressure, retry policy, pin model+dimension+version vào index signature — pattern desktop đã có) → upsert Qdrant + insert `chunks` FTS, checkpoint per-batch
4. Worker scale ngang; trạng thái từng bước realtime lên UI.

### Q&A pipeline
Embed query → Qdrant top-k (filter org + collection ACL) ∥ PG FTS (`unaccent`+`simple`) → merge/rerank theo công thức hybrid sẵn trong knowledge.rs → prompt kèm nguồn → GLM stream SSE → answer + citation (doc, heading path, link; citation fetch re-check ACL + trạng thái tài liệu). LLM lỗi → fallback trả trích đoạn.

### RBAC thực thi — defense-in-depth (finding #3)
- Extractor `AuthUser` (JWT → membership+permissions, cache TTL ngắn); guard per-route `require(perm)`.
- **Tenant-scoped repository**: tầng data access chỉ expose API nhận `OrgContext` — không thể viết query thiếu `org_id` (cân nhắc thêm PG RLS làm lưới thứ hai).
- Qdrant adapter **từ chối search** nếu thiếu `org_id + allowed_collection_ids`.
- **Denial test bắt buộc** phủ: Qdrant search, PG FTS, citation fetch, preview/download.

### Rate limit — atomic (finding #6)
- Request: tower_governor per-user (per-IP khi chưa auth), auth endpoints chặt hơn; in-memory 1 node, Redis chỉ khi multi-replica.
- Quota: **reserve → finalize/refund** atomic trong PG (bảng `quota_reservations`) thay vì check-then-act; semaphore concurrent job theo org/user; đếm LLM token thật từ response usage; 429 + quota headers; dashboard usage cho org admin (bản tối thiểu ở web MVP).

### Draft SLA targets (chốt số ở planning)
- Query: retrieval < 500ms P95; first-token Q&A < 5s P95.
- Ingest: throughput đo & công bố theo worker-node (benchmark Phase 0 quyết định số); job resume sau restart ≤ 1 checkpoint.
- Recovery: PG backup + Qdrant snapshot định kỳ; RPO theo lịch snapshot, index rebuild được từ PG (chunks là nguồn sự thật).

## 6. Phasing (đã sửa theo Codex review — round 1 cũ quá rộng)

| Phase | Deliverable | Gate |
|---|---|---|
| **0** | Spike scale/security: benchmark Qdrant+PG FTS với phân bố org thật; eval embedding golden-set tiếng Việt (bge-m3 vs e5); upload threat model; chốt SLA số | Số liệu đạt ngưỡng mới đi tiếp |
| **1A** | Tách `crates/knowledge` — desktop behavior giữ nguyên, test pass | `cargo test` + desktop chạy đúng |
| **1B** | Server vertical slice **single-org (POC)**: auth JWT đơn giản, upload (hardening đầy đủ) → convert → index → Q&A citation, state machine + reconciliation | Demo POC 1 đơn vị, vài account test |
| **1C** | Multi-org đầy đủ: RBAC mức 2 + ACL + quota atomic + denial tests | Denial test suite pass |
| **2** | Web SPA MVP: login, thư viện (collection/upload/preview/xóa/reindex), chat Q&A citation, admin tối thiểu (member/role/usage) | End-to-end demo |
| **3** | Port intelligence: tóm tắt, quality, PII/redaction, BA/PM handoff | |
| **4** | Hardening sâu (audit review, pentest checklist), SSO/OIDC, trang hướng dẫn sử dụng + onboarding | |

POC hiện tại = Phase 0 → 1B. Schema multi-org từ đầu (org_id mọi bảng) nên 1C không cần migration phá.

## 7. Rủi ro

1. [Unverified] Chất lượng embedding tiếng Việt (bge-m3) trên corpus thật — Phase 0 eval golden-set **trước khi ingest hàng loạt**.
2. Ingest lớn: OCR cổ chai CPU (1-5s/trang scan) — worker scale ngang + dự trù phần cứng; embedding queue có backpressure.
3. Refactor 5.200 dòng gắn Tauri+SQLite+filesystem+sync-blocking — Phase 1A riêng, test chốt hành vi trước.
4. GPU server chưa xác nhận cấu hình — fallback: embedding API hoặc hash-local (giảm chất lượng).
5. Consistency 3 hệ (PG/Qdrant/MinIO) — đã thiết kế state machine + idempotency + tombstone + reconciliation; vẫn là nơi dễ bug nhất, cần test kỹ.
6. PhoWhisper license chưa rõ — kiểm tra trước khi bundle vào deploy web.

## 8. Success metrics

- Desktop build/test pass sau tách crate (hành vi không đổi).
- Phase 0 gate: benchmark + eval đạt ngưỡng thống nhất.
- Ingest end-to-end 1 tài liệu mỗi định dạng hỗ trợ → query ra citation đúng; kill worker giữa chừng → job resume đúng checkpoint.
- Denial tests: user ngoài collection không nhận nội dung qua Q&A/FTS/citation/preview/download.
- Quota: vượt quota → 429; 2 request đồng thời không vượt được quota (reserve atomic).
- Upload độc hại (extension giả, zip-bomb, file quá size) bị chặn trước converter.

## 9. Next steps

1. `/ck:plan` cho POC (Phase 0 → 1B), input = report này.
2. Xác nhận hạ tầng: GPU server (VRAM → chọn model embed), provisioning PG/Qdrant/MinIO, GLM endpoint.
3. Chuẩn bị golden-set câu hỏi-đáp tiếng Việt từ tài liệu mẫu cho Phase 0.

## Unresolved questions

- Cấu hình GPU server thực tế (VRAM quyết định bge-m3 hay e5-small).
- Số SLA cụ thể (latency/throughput/recovery đều cần — user xác nhận quan tâm cả 3; chốt số ở planning sau benchmark Phase 0).
- Danh sách định dạng upload cho phép ở POC (đủ bộ core hỗ trợ hay giới hạn pdf/docx/xlsx trước?).
