# Markhand Web — Tài liệu thiết kế hệ thống

> Chuyển thể từ brainstorm đã APPROVED
> ([`../plans/reports/brainstorm-260713-1656-markhand-web-rag-multi-org-report.md`](../plans/reports/brainstorm-260713-1656-markhand-web-rag-multi-org-report.md),
> đã qua Codex review 8 finding) thành tài liệu thiết kế chuẩn cho team đọc trước khi nhận task.
> Phasing & giao việc: [`../plans/260713-markhand-web-phase-plan.md`](../plans/260713-markhand-web-phase-plan.md).
> Mục **Chiến lược kiểm thử** là đề xuất mới (chưa có trong brainstorm gốc), đánh dấu [Đề xuất].

## 1. Bài toán & phạm vi

Web app quản lý tài liệu trên nền `fileconv-core`: upload → trích xuất
(pdf/docx/pptx/xlsx/csv/html/txt + OCR + audio → Markdown) → embed → RAG hỏi đáp
có citation, kèm thư viện tài liệu, intelligence và trang hướng dẫn.
Multi-org, multi-user, on-prem, phân quyền chức năng + rate limit.

**Quy mô chốt**: mỗi org ~vài trăm GB ([Inference] ≈ 10–20M chunk/org), tổng vài TB.
**Giai đoạn hiện tại**: POC 1 đơn vị, vài tài khoản test — schema multi-org từ đầu,
triển khai theo lát cắt dọc single-org trước.

## 2. Quyết định kiến trúc (đã chốt với user)

| Hạng mục | Quyết định |
|---|---|
| Backend | Rust, axum; tách crate dùng chung với desktop |
| Storage | **PG (system-of-record) + Qdrant (vector) + MinIO (file gốc)** |
| Embedding | **POC: GLM embedding API** (chưa có GPU — quy mô POC chấp nhận chi phí/độ trễ API). Khi có GPU chuyển vLLM local (bge-m3/e5): index signature pin model+dimension nên đổi model chỉ cần reindex. Gate: eval GLM embedding trên golden-set tiếng Việt trước khi ingest hàng loạt |
| Chat LLM | GLM cloud (user xác nhận chấp nhận gửi nội dung tài liệu; giữ option self-host) |
| Upload POC | `docx, xlsx, pdf (text + scan), csv, md, txt, ảnh (OCR)`. **Từ chối `.doc` binary cũ với thông báo rõ** (core chỉ hỗ trợ docx; user tự convert sang docx). `md/txt` lưu pass-through qua decode `viet_legacy`. Chưa nhận audio ở POC |
| Auth | JWT tự build cho POC; auth layer tách riêng, pluggable OIDC (SSO tương lai) |
| RBAC | Mức 2: role per-org (owner/admin/editor/viewer) + ACL collection; schema role→permissions dạng bảng để nâng custom-role sau |
| Rate limit | 2 tầng: request (tower_governor) + quota tài nguyên (LLM token, upload GB, concurrent jobs) |
| Frontend | React + Vite SPA mới trong `web/`, serve static từ axum; **tái sử dụng token/theme LumiBase + component pattern desktop** (SafeMarkdown, ui.tsx) |
| Desktop | Giữ nguyên SQLite FTS5 hybrid — không đổi flow desktop |
| Deploy | On-prem nội bộ |

Phương án đã loại: pgvector partition per-org (biên khả năng ở 10–20M chunk/org),
OpenSearch (vận hành JVM nặng on-prem), copy logic RAG thành bản riêng (2 nguồn sự thật),
web qua MCP (không hợp upload lớn/SSE/session).

## 3. Bố cục workspace

```
crates/core       — giữ nguyên (convert, chunk, llm, viet_legacy…)
crates/knowledge  — MỚI: tách từ app/src-tauri/{knowledge,vector_index,intelligence}.rs
                    Extract tối thiểu theo use-case: logic thuần chunk→embed→rank→citation
                    trước; KHÔNG generic hoá storage sớm (Codex finding). Desktop giữ
                    SQLite/HNSW, hành vi không đổi (test khoá trước khi tách).
crates/server     — MỚI: axum. Auth module tách riêng (pluggable OIDC), RBAC middleware,
                    tower_governor + quota, REST + SSE, worker pool,
                    adapter PG/Qdrant/MinIO.
web/              — MỚI: React+Vite SPA; thay ipc.ts bằng HTTP client;
                    tái dùng SafeMarkdown/pattern view/LumiBase token.
```

Hiện trạng tái dùng (scout từ brainstorm): ~80% logic RAG đã có trong desktop
(`knowledge.rs` 1.672 dòng FTS5+vector hybrid+citation, `vector_index.rs` HNSW persistent,
`intelligence.rs` 1.168 dòng) nhưng gắn Tauri + SQLite + sync-blocking I/O —
bài toán thực là **tách lớp + bọc axum + web UI + multi-tenancy**, không phải build mới.

## 4. PG schema (mọi bảng nghiệp vụ có `org_id`)

> **DDL chi tiết từng bảng/cột/index + state machine transition + seed permission:**
> [`web-db-schema.md`](web-db-schema.md) — spec trực tiếp cho task 1.7/2.2/2.7/2.8.

- **Tenancy**: `orgs`, `users`, `org_memberships(role)`, `refresh_tokens`
- **RBAC**: `roles(org_id nullable)`, `role_permissions`; permission string trong code:
  `doc.upload | doc.delete | qa.query | member.manage | settings.manage | intel.use | audit.view | export.run`…
- **Docs**: `collections(visibility: private|org|groups)`, `collection_access`,
  `documents(status, content_hash BLAKE3, minio_key)`, `document_versions`,
  `chunks(heading_path, text, tsv)` partition theo `org_id`.
  **Content hash dùng BLAKE3/SHA-256** — không dùng `DefaultHasher` như desktop
  (không ổn định xuyên version/process).
- **Jobs**: `jobs(type, status, attempts, locked_by, checkpoint, idempotency_key)` —
  claim bằng `FOR UPDATE SKIP LOCKED`, resumable theo checkpoint.
- **Quota**: `org_quotas`, `usage_counters`, `quota_reservations` (xem mục 9).
- **`audit_log`**.

## 5. Document state machine & consistency PG/Qdrant/MinIO

Trạng thái: `uploaded → converting → converted → indexing → indexed | failed`.

- **PG là nguồn sự thật**; mọi bước ghi Qdrant/MinIO đều idempotent
  (idempotency key theo document version + batch).
- **Delete = tombstone**: đánh dấu PG trước → job xóa vector Qdrant + object MinIO;
  citation fetch luôn re-check trạng thái PG → tài liệu đã xóa/thu hồi quyền không bao giờ trả ra.
- **Reconciliation job** định kỳ: quét lệch PG↔Qdrant↔MinIO (orphan file, stale vector) và sửa.

Đây là vùng dễ bug nhất toàn hệ thống (risk #5) — test kỹ nhất.

## 6. Qdrant

1 collection chung; payload `{org_id, collection_id, document_id, chunk_id}` +
payload index org/collection; scalar quantization; text lấy từ PG theo `chunk_id`.
**Phase 0 bắt buộc benchmark**: payload filter latency, delete/update payload,
snapshot/restore, RAM sau quantization với phân bố org thật — cấu hình collection
(dimension, quantization params) chỉ chốt sau gate này.

## 7. Ingest pipeline (hardening ngay từ vertical slice — KHÔNG để phase cuối)

1. **Upload multipart**: MIME sniff magic-byte đối chiếu extension (core suy định dạng
   theo extension nên web phải có lớp bảo vệ riêng), giới hạn size/page/duration,
   chống zip-bomb, quarantine bucket → quota **reserve** → MinIO → `documents(uploaded)` + job `convert`.
2. **Worker convert**: fileconv-core với **timeout + kill**, process cách ly (sandbox worker);
   PDF 3-tier/OCR/audio như hiện tại → Markdown → job `index`.
3. **Index**: chunk heading-path → **embedding queue riêng** (batch scheduler, backpressure,
   retry policy, pin model+dimension+version vào index signature — pattern desktop đã có)
   → upsert Qdrant + insert `chunks` FTS, checkpoint per-batch.
4. Worker scale ngang; trạng thái từng bước realtime lên UI.

## 8. Q&A pipeline

Embed query → Qdrant top-k (filter `org_id` + collection ACL) ∥ PG FTS (`unaccent`+`simple`)
→ merge/rerank theo công thức hybrid sẵn trong knowledge.rs → prompt kèm nguồn
→ GLM stream SSE → answer + citation (doc, heading path, link;
citation fetch re-check ACL + trạng thái tài liệu). LLM lỗi → fallback trả trích đoạn.

**Guardrail chống ảo giác** (nguyên tắc "open-book exam"): system prompt yêu cầu chỉ trả
lời từ context được cấp; không đủ căn cứ → trả "không tìm thấy trong tài liệu" (kèm chunk
gần nhất làm gợi ý), không suy diễn ngoài nguồn. Golden-set có bộ câu hỏi negative
(`no_answer`) để đo refusal accuracy — từ chối đúng khi không có, không từ chối nhầm khi có.

## 9. RBAC, tenant isolation & rate limit

**Defense-in-depth (áp dụng từ ngày đầu Integration, không chờ phase multi-org):**
- Extractor `AuthUser` (JWT → membership+permissions, cache TTL ngắn); guard per-route `require(perm)`.
- **Tenant-scoped repository**: tầng data-access chỉ expose API nhận `OrgContext` —
  không thể viết query thiếu `org_id`; xây MỘT LẦN ở Integration, dùng ngay cả khi POC 1 org
  (cân nhắc PG RLS làm lưới thứ hai).
- Qdrant adapter **từ chối search** nếu thiếu `org_id + allowed_collection_ids`.
- **Denial test bắt buộc** phủ: Qdrant search, PG FTS, citation fetch, preview/download.

**Rate limit & quota (atomic — một cơ chế duy nhất, không chia giai đoạn):**
- Request: tower_governor per-user (per-IP khi chưa auth), auth endpoints chặt hơn;
  in-memory 1 node, Redis chỉ khi multi-replica.
- Quota: **reserve → finalize/refund atomic trong PG** (bảng `quota_reservations`)
  thay vì check-then-act; semaphore concurrent job theo org/user; đếm LLM token thật
  từ response usage; 429 + quota headers; dashboard usage cho org admin.

## 10. Frontend web

- React + Vite SPA trong `web/`, TypeScript strict, Zustand (pattern store desktop).
- **Tái sử dụng LumiBase**: port token màu/spacing/dark theme từ `app/styles.css`;
  tái dùng `SafeMarkdown`, pattern `LibraryView`/`IntelligenceView`, `ui.tsx`.
  Bề mặt preview file nguồn giữ nền sáng như desktop.
- Màn hình MVP: login → thư viện (collection/upload/preview/xóa/reindex, trạng thái
  ingest realtime) → chat Q&A citation (SSE stream) → admin tối thiểu (member/role/usage).

## 11. Chiến lược kiểm thử [Đề xuất — mới, chưa có trong brainstorm gốc]

Track chạy **xuyên suốt**, không phải phase cuối:

1. **Unit/integration test theo crate**: `crates/knowledge` khoá hành vi trước khi tách
   (điều kiện gate 1A); `crates/server` test adapter + state machine + quota atomic
   (2 request đồng thời không vượt quota); reconciliation test kill-worker-giữa-chừng.
2. **Denial test suite** (bắt buộc trước rollout multi-org): user ngoài collection
   không nhận nội dung qua Q&A/FTS/citation/preview/download.
3. **Upload adversarial tests**: extension giả, zip-bomb, file quá size — chặn trước converter.
4. **Playwright E2E cho web SPA**: kịch bản login → upload → theo dõi trạng thái
   convert/index → hỏi đáp → verify citation link mở đúng tài liệu. Chạy trong CI
   với docker-compose (PG/Qdrant/MinIO) + embedding hash-local fallback để không cần GPU.
5. **Agent-driven browser test**: dùng AI agent điều khiển browser (Playwright MCP)
   chạy exploratory smoke test theo kịch bản ngôn ngữ tự nhiên trên staging —
   bổ sung cho E2E script cứng, bắt lỗi UX/flow mà script không phủ.
   Chi tiết phạm vi + tiêu chí pass/fail chốt ở bước chia task.

## 12. Draft SLA targets (chốt số ở planning sau benchmark Phase 0)

- Query: retrieval < 500ms P95; first-token Q&A < 5s P95.
- Ingest: throughput đo & công bố theo worker-node; job resume sau restart ≤ 1 checkpoint.
- Recovery: PG backup + Qdrant snapshot định kỳ; RPO theo lịch snapshot;
  index rebuild được từ PG (chunks là nguồn sự thật).

## 13. Rủi ro

| # | Rủi ro | Gắn với |
|---|---|---|
| 1 | [Unverified] Chất lượng embedding tiếng Việt (bge-m3) trên corpus thật | Gate Phase 0 — eval golden-set trước khi ingest hàng loạt |
| 2 | OCR cổ chai CPU (1–5s/trang scan) | Ingest/RAG — worker scale ngang + backpressure |
| 3 | Refactor 5.200 dòng gắn Tauri+SQLite+sync-blocking | Track Core reuse — test khoá hành vi trước |
| 4 | GPU server chưa xác nhận cấu hình | Phase 0 + RAG — fallback embedding API hoặc hash-local (giảm chất lượng) |
| 5 | Consistency 3 hệ PG/Qdrant/MinIO | Integration — state machine + reconciliation, vẫn là nơi dễ bug nhất |
| 6 | PhoWhisper license chưa rõ | Kiểm tra trước khi bundle audio vào deploy web |

## 14. Câu hỏi chưa chốt

- GLM endpoint + API key provisioning (điều kiện bắt đầu eval embedding).
- Số SLA cụ thể (latency/throughput/recovery — chốt sau benchmark Track A).
- Thời điểm/cấu hình GPU server cho vLLM (chuyển đổi từ GLM embedding API về local).

Đã chốt (2026-07-13): embedding POC dùng GLM API; danh sách format upload POC (mục 2);
từ chối `.doc`; quy ước code server+web đặt ở file riêng `docs/web-code-standards.md`.

## 15. API surface v1 (dự kiến — chốt dần theo task, prefix `/api`)

| Nhóm | Endpoint | Task |
|---|---|---|
| Auth | `POST /auth/login` · `POST /auth/refresh` · `POST /auth/logout` · `GET /me` | 2.3 |
| Collections | `GET/POST /collections` · `PATCH/DELETE /collections/:id` · `PUT /collections/:id/access` | 2.2, 5.2 |
| Documents | `POST /documents` (multipart) · `GET /documents?collection=` · `GET /documents/:id` · `GET /documents/:id/markdown` · `GET /documents/:id/status` (poll/SSE) · `POST /documents/:id/reindex` · `DELETE /documents/:id` (tombstone) | 2.5, 2.6, 4.2, 5.2 |
| Q&A | `POST /qa` (SSE stream answer+citations) · `GET /chunks/:id` (citation fetch — re-check ACL + tombstone) | 3.4 |
| Intelligence | `POST /documents/:id/summarize` · `POST /documents/:id/pii` | 5.3 |
| Admin | `GET/PATCH /org/members` · `GET /org/usage` · `GET/PATCH /org/quotas` | 5.1, 6.2 |
| Hệ thống | `GET /healthz` (ngoài prefix, không auth) | 1.6 |

Quy ước: JSON snake_case theo serde mặc định của struct Rust; lỗi trả
`{error: {code, message}}` thống nhất từ error type chung (1.6); mọi route sau auth
đi qua guard `require(perm)` (2.3/6.1); SSE cho stream (Q&A, trạng thái ingest).

## Tham chiếu chéo

- Brainstorm gốc: [`../plans/reports/brainstorm-260713-1656-markhand-web-rag-multi-org-report.md`](../plans/reports/brainstorm-260713-1656-markhand-web-rag-multi-org-report.md)
- Phasing & giao việc: [`../plans/260713-markhand-web-phase-plan.md`](../plans/260713-markhand-web-phase-plan.md)
- Kiến trúc desktop hiện tại: [`system-architecture.md`](system-architecture.md)
- Quy ước code: [`code-standards.md`](code-standards.md)
