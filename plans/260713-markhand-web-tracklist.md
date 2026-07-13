# Markhand Web — Track list (map GitHub issue)

> Nguồn: [`260713-markhand-web-phase-plan.md`](260713-markhand-web-phase-plan.md) (lịch 45 ngày)
> + [`260713-markhand-web-task-breakdown.md`](260713-markhand-web-task-breakdown.md) (hướng dẫn mở rộng Phase 1-2).
> Mỗi task dưới đây = 1 GitHub issue. Khi tạo issue: title = `[P<phase>] <id> <tên task>`,
> body copy nguyên khối task (Hướng làm + Việc cần làm + DoD), gán label + milestone như ghi chú,
> rồi điền số issue vào cột Issue ở bảng tổng.

## Quy ước theo dõi

- **Milestone GitHub** = M1…M7 (đúng gate trong phase plan). Issue thuộc phase nào gán milestone đó.
- **Label**: `phase-1`…`phase-7` + nhóm (`infra`, `rust-core`, `server`, `frontend`, `qa`, `security`, `docs`).
- **Trạng thái** cột cuối bảng tổng: `⬜ chưa làm / 🟨 đang làm / ✅ xong (đã qua DoD) / ⛔ blocked`.
- Task của Lead+AI vẫn tạo issue để theo dõi tiến độ chung.
- DoD (Definition of Done) là gate nhị phân — chưa đủ DoD thì không đóng issue.

## Bảng tổng

| ID | Task | Ai | Ngày | Phụ thuộc | Issue | TT |
|---|---|---|---|---|---|---|
| 1.1 | Docker-compose PG/Qdrant/MinIO | Member 1 | 1-2 | máy dev | — | ⬜ |
| 1.2 | `docs/web-code-standards.md` | M2+M3 | 1-3 | — | — | ⬜ |
| 1.3 | Test khoá hành vi knowledge/vector_index | Lead+AI | 1-4 | — | — | ⬜ |
| 1.4 | Skeleton `web/` + LumiBase + lint | Member 3 | 1-6 | 1.2 (phần web) | — | ⬜ |
| 1.5 | Golden-set tiếng Việt rút gọn | Member 1 | 3-6 | 1.1 | — | ⬜ |
| 1.6 | Skeleton `crates/server` | Member 2 | 3-6 | 1.2 (phần server) | — | ⬜ |
| 1.7 | PG schema migration + seed | Member 2 | 4-8 | 1.1, 1.6 | — | ⬜ |
| 1.8 | Threat model upload | Member 1 | 4-5 | — | — | ⬜ |
| 1.9 | Eval GLM embedding | Member 1 | 6-8 | 1.5, GLM key | — | ⬜ |
| 1.10 | Benchmark Qdrant lite | Member 1 | 6-9 | 1.1 | — | ⬜ |
| 2.1 | Tách `crates/knowledge` | Lead+AI | 5-13 | 1.3 | — | ⬜ |
| 2.2 | Tenant-scoped repository | Lead+AI | 9-12 | 1.7 | — | ⬜ |
| 2.3 | Auth JWT + refresh | Lead+M2 | 10-13 | 1.7, 2.2 | — | ⬜ |
| 2.4 | Qdrant collection + adapter | Member 2 | 10-12 | 1.9, 1.10, 2.2 | — | ⬜ |
| 2.5 | Upload API + hardening | Member 2 | 11-16 | 1.8, 2.2, 2.3 | — | ⬜ |
| 2.6 | Jobs queue + worker convert | Lead+AI | 12-16 | 1.7, 2.5 | — | ⬜ |
| 2.7 | State machine + reconciliation | Lead+AI | 14-19 | 2.6 | — | ⬜ |
| 2.8 | Quota atomic | Lead+AI | 16-18 | 1.7, 2.5 | — | ⬜ |
| 2.9 | Integration test M2 | Member 1 | 14-19 | 2.5-2.8 | — | ⬜ |
| 3.1 | Embedding queue GLM | Lead+AI | 16-20 | 2.1, 2.4, 1.9 | — | ⬜ |
| 3.2 | Index step (Qdrant + FTS) | M2+Lead | 18-22 | 3.1, 2.6 | — | ⬜ |
| 3.3 | Hybrid search + rerank | Lead+AI | 20-24 | 2.1, 3.2 | — | ⬜ |
| 3.4 | Q&A endpoint SSE + citation | Lead+AI | 22-26 | 3.3 | — | ⬜ |
| 3.5 | Eval harness recall | Member 1 | 24-26 | 3.4, 1.5 | — | ⬜ |
| 4.1 | UI Login + route guard | Member 3 | 20-22 | 1.4, 2.3 | — | ⬜ |
| 4.2 | UI Thư viện + upload + trạng thái | Member 3 | 22-27 | 4.1, 2.5 | — | ⬜ |
| 4.3 | UI Chat Q&A citation | Member 3 | 25-30 | 4.1, 3.4 | — | ⬜ |
| 4.4 | Playwright E2E smoke | Member 1 | 27-31 | 4.2, 4.3 | — | ⬜ |
| 4.5 | Demo POC tổng duyệt | Cả team | 30-32 | 4.4 | — | ⬜ |
| 5.1 | UI Admin member/role/usage | Member 3 | 30-34 | 4.5 | — | ⬜ |
| 5.2 | Collection management + reindex UI | Member 3 | 33-37 | 5.1 | — | ⬜ |
| 5.3 | Intelligence rút gọn (tóm tắt + PII) | Lead+AI | 30-36 | 4.5 | — | ⬜ |
| 5.4 | Mở rộng Playwright E2E | Member 1 | 32-38 | 5.1-5.3 | — | ⬜ |
| 6.1 | RBAC role/ACL mức 2 đầy đủ | Lead+M2 | 32-38 | 2.2, 2.3, 4.5 | — | ⬜ |
| 6.2 | Rate limit 2 tầng + quota dashboard | M2+Lead | 35-40 | 2.8, 6.1 | — | ⬜ |
| 6.3 | Denial test suite | Member 1 | 36-42 | 6.1 | — | ⬜ |
| 6.4 | Rollout multi-org demo 2 org | Cả team | 40-42 | 6.1-6.3 | — | ⬜ |
| 7.1 | Backup/recovery drill | Member 1 | 40-43 | 6.4 | — | ⬜ |
| 7.2 | Security checklist nội bộ + audit | Lead | 40-44 | 6.4 | — | ⬜ |
| 7.3 | Trang hướng dẫn + onboarding | Member 3 | 40-44 | 5.2 | — | ⬜ |
| 7.4 | Auth interface OIDC-ready | Lead | 43-45 | 6.1 | — | ⬜ |

---

## Phase 1 — Nền tảng & Spike (→ milestone M1)

### 1.1 Docker-compose PG/Qdrant/MinIO — `infra`
**Hướng làm:** Compose 3 service trong `bench/web-spike/`: PostgreSQL 16 (init script bật `unaccent`), Qdrant, MinIO. Pin image tag cụ thể, named volume, healthcheck từng service, `.env.example` (không commit secret).
**Việc cần làm:**
- [ ] `docker-compose.yml` + init script PG + `.env.example`
- [ ] `README.md`: lệnh up/down/reset volume
- [ ] Kiểm tra port không đụng dải đang dùng
**DoD:** `docker compose up -d` → 3 service healthy; PG có `unaccent`; MinIO console vào được.

### 1.2 Quy ước code server+web — `docs`
**Hướng làm:** Thống nhất khung mục lục trước. M2 viết phần server: layout module axum, error type (thiserror + IntoResponse), quy ước sqlx/migration, async (convert qua `spawn_blocking`/worker — KHÔNG block runtime), tracing. M3 viết phần web: kế thừa chuẩn app desktop (TS strict, PascalCase component, Zustand store duy nhất), quy ước API client tập trung, quản lý token.
**Việc cần làm:**
- [ ] Khung mục lục (2 người chốt chung)
- [ ] Phần server (M2) · Phần web (M3)
- [ ] Lead review + merge
**DoD:** lead duyệt; PR sau đó bị review theo file này. Chỉ ghi quy ước dự án này cần (YAGNI), không chép nguyên tắc chung chung.

### 1.3 Test khoá hành vi knowledge/vector_index — `rust-core`
**Hướng làm:** Viết test cấp hành vi cho `app/src-tauri/src/{knowledge,vector_index}.rs` chạy không cần Tauri runtime: index → hybrid search → citation đúng; incremental content hash; HNSW persist/reload. Mục tiêu: lưới an toàn cho 2.1, fail nếu logic rank/citation đổi.
**Việc cần làm:**
- [ ] Xác định bề mặt hành vi cần khoá (search kết quả + thứ hạng, citation anchor, hash ổn định)
- [ ] Fixture corpus nhỏ tiếng Việt trong test
- [ ] Test pass trên code HIỆN TẠI
**DoD:** `cargo test` pass; đổi thử công thức rank → test fail (chứng minh test đủ nhạy).

### 1.4 Skeleton `web/` — `frontend`
**Hướng làm:** Vite + React + TS strict (mirror config `app/`), routing 4 trang placeholder (login/thư viện/chat/admin), HTTP client tập trung `web/src/lib/api.ts` (vai trò như `ipc.ts`), Zustand store skeleton (auth + tree placeholder). Copy `SafeMarkdown` (thuần React). Port token LumiBase từ `app/src/styles.css` giữ nguyên tên biến; copy ESLint/Prettier config từ `app/`.
**Việc cần làm:**
- [ ] Scaffold Vite + routing + store + api client
- [ ] Port LumiBase token + copy SafeMarkdown
- [ ] ESLint/Prettier + CI job lint cho `web/`
**DoD:** `pnpm dev` chạy, điều hướng 4 trang, gọi `/healthz` hiển thị trạng thái; lint pass. KHÔNG import `@tauri-apps/*`; không tạo package chung app/web.

### 1.5 Golden-set tiếng Việt rút gọn — `qa`
**Hướng làm:** 15-20 tài liệu mẫu đúng danh sách format POC (docx/xlsx/pdf text/pdf scan/csv/md/txt/ảnh) → convert `fileconv one` → chunk heading-path → 300-500 chunk. Soạn 30-50 cặp câu hỏi + chunk nguồn đúng (manifest TSV, `#` comment — cùng convention accuracy bench).
**Việc cần làm:**
- [ ] Chọn tài liệu phủ loại khó (scan, bảng, IN HOA — theo `bench/REPORT_EDGE.md`)
- [ ] Convert + chunk + đánh id chunk
- [ ] Soạn Q/A: câu hỏi viết như người dùng thật (KHÔNG copy nguyên văn trong tài liệu)
- [ ] Gitignore data nhạy cảm, chỉ commit manifest + script
**DoD:** `bench/web-spike/golden/` có manifest + chunk chuẩn hoá; lead duyệt độ phủ.

### 1.6 Skeleton `crates/server` — `server`
**Hướng làm:** Thêm workspace member `crates/server`: axum + tokio, config từ env (fail-fast), error type chung theo mẫu `ConvertError`, `/healthz` check PG+Qdrant+MinIO reachable, tracing. Layout module theo 1.2. CHƯA business logic.
**Việc cần làm:**
- [ ] Thêm crate vào workspace (kiểm tra `vendor/markitdown-rs` vẫn exclude)
- [ ] Config env + error type + tracing
- [ ] `/healthz` + CI build
**DoD:** `cargo run -p fileconv-server` → `/healthz` trả trạng thái 3 service từ 1.1; không phá `cargo test` hiện có; không kéo dependency "cho tương lai".

### 1.7 PG schema migration + seed — `server`
**Hướng làm:** Migration sqlx theo `docs/web-architecture.md` mục 4 (tenancy, RBAC, docs, jobs, quota, audit — mọi bảng nghiệp vụ có `org_id`). Làm từng nhóm bảng một PR nhỏ để lead review kịp. Seed dev: 1 org, 2 user, 1 collection.
**Việc cần làm:**
- [ ] Nhóm tenancy + RBAC · nhóm docs/chunks (tsvector + GIN, partition-ready org_id) · nhóm jobs/quota/audit
- [ ] `content_hash` cột text/bytea (BLAKE3 hex — KHÔNG bigint) · `jobs.idempotency_key` unique
- [ ] Seed script
**DoD:** `sqlx migrate run` sạch trên PG của 1.1; seed chạy; lead đối chiếu từng bảng với design doc.

### 1.8 Threat model upload — `security`
**Hướng làm:** Liệt kê bề mặt tấn công theo format POC: extension giả, zip-bomb trong docx/xlsx (bản chất là zip), decompression bomb ảnh, PDF malformed panic (lopdf — lý do core bọc catch_unwind), quá size, path traversal tên file. Mỗi mục: cách chặn + tầng chặn (trước MinIO / trước converter / trong sandbox).
**Việc cần làm:**
- [ ] Bảng threat → mitigation → tầng chặn
- [ ] Chuẩn bị sample file độc (zip-bomb nhỏ, extension giả) cho test 2.5/2.9
**DoD:** lead duyệt; là input trực tiếp của 2.5.

### 1.9 Eval GLM embedding — `qa` ⛔ chặn bởi GLM key
**Hướng làm:** Script embed toàn bộ chunk + câu hỏi golden-set qua GLM API (OpenAI-compatible — nhất quán cách gọi với `crates/core/src/llm.rs`) → cosine top-k → recall@1/5/10. Baseline: hash-local 256D. Ghi chi phí token + latency per-batch. Cache kết quả embed ra file (chạy lại không tốn tiền).
**Việc cần làm:**
- [ ] Script eval + cache
- [ ] Batch nhỏ + retry theo rate limit
- [ ] `bench/REPORT_WEB_EMBEDDING.md`: bảng recall + chi phí + latency
**DoD:** report đủ số liệu; lead + user chốt đạt/không đạt (**GATE M1**); nếu fail → mở issue đổi provider ngay.

### 1.10 Benchmark Qdrant lite — `infra`
**Hướng làm:** Nạp ~1M vector synthetic (dimension theo model GLM dự kiến) chia ~10 org **phân bố lệch** (1 org chiếm ~50% — phân bố đều cho kết quả đẹp giả tạo), bật scalar quantization + payload index → đo search filter P95, RAM trước/sau quantization. Snapshot/restore đầy đủ để sang 7.1.
**Việc cần làm:**
- [ ] Script nạp + đo · [ ] `bench/REPORT_WEB_QDRANT.md` + đề xuất config collection cho 2.4
**DoD:** report có số P95 + RAM; đề xuất config được 2.4 dùng trực tiếp.

## Phase 2 — Tách core & Integration (→ M2)

### 2.1 Tách `crates/knowledge` — `rust-core`
**Hướng làm:** Extract logic thuần chunk→rank→citation + types chung từ `app/src-tauri/src/{knowledge,vector_index,intelligence}.rs` sang crate mới. KHÔNG generic hoá storage (Codex finding): desktop giữ SQLite/HNSW tại chỗ, chỉ phần thuần dời đi; desktop compile lại trỏ sang crate mới.
**Việc cần làm:**
- [ ] Khoanh vùng phần thuần vs phần dính SQLite/Tauri/filesystem
- [ ] Dời types + cấu trúc rank/RRF/citation · [ ] Desktop dùng lại từ crate mới
- [ ] Interface embed/store tối thiểu (chỉ khi 3.1/2.4 cần — không đoán trước)
**DoD:** `cargo test` toàn workspace pass + test khoá 1.3 pass nguyên trạng + desktop chạy đúng (**gate track**).

### 2.2 Tenant-scoped repository — `server` `security`
**Hướng làm:** Tầng data-access duy nhất của server: mọi API nhận `OrgContext`, không thể viết query thiếu `org_id`. Mọi call-site sau (2.3-2.8, 3.x) bắt buộc đi qua tầng này. Cân nhắc PG RLS làm lưới thứ hai (ghi lại quyết định trong PR).
**Việc cần làm:**
- [ ] Struct `OrgContext` + repo trait/impl cho documents/collections/jobs/quota
- [ ] Test: query không có org đúng → không lấy được dữ liệu org khác
**DoD:** không còn đường query PG trực tiếp ngoài repo layer; test cách ly pass.

### 2.3 Auth JWT + refresh — `server`
**Hướng làm:** Module auth tách riêng (pluggable OIDC sau — 7.4): login → access JWT ngắn hạn + refresh token (bảng `refresh_tokens`), extractor `AuthUser` (membership + permissions, cache TTL ngắn), middleware guard `require(perm)`.
**Việc cần làm:**
- [ ] Endpoint login/refresh/logout (lead làm khung, M2 làm endpoint phụ)
- [ ] Extractor + guard · [ ] Hash mật khẩu chuẩn (argon2/bcrypt)
**DoD:** flow login→gọi API→refresh→logout chạy; route thiếu perm trả 403; token hết hạn trả 401.

### 2.4 Qdrant collection + adapter — `server`
**Hướng làm:** Init collection theo đề xuất 1.10 (dimension theo model chốt ở 1.9, scalar quantization, payload index `org_id`/`collection_id`). Adapter **từ chối search** nếu thiếu `org_id + allowed_collection_ids`.
**Việc cần làm:**
- [ ] Script/module init idempotent · [ ] Adapter upsert/search/delete theo filter
- [ ] Test: search thiếu org context → error, không phải kết quả rỗng
**DoD:** upsert + search filter org chạy trên compose; test từ chối pass.

### 2.5 Upload API + hardening — `server` `security`
**Hướng làm:** Multipart upload theo đúng thứ tự: MIME sniff magic-byte đối chiếu extension (từ chối `.doc` với message hướng dẫn convert docx) → size limit → zip-bomb check (docx/xlsx cũng là zip!) → quota reserve (2.8) → quarantine bucket MinIO → `documents(uploaded)` + job convert. Làm thành chuỗi PR nhỏ theo từng lớp chặn (thứ tự theo threat model 1.8).
**Việc cần làm:**
- [ ] Endpoint multipart + validate từng lớp (mỗi lớp 1 PR)
- [ ] Test bằng sample độc từ 1.8
**DoD:** mọi sample độc bị chặn trước converter với error message rõ; file sạch vào MinIO + tạo job.

### 2.6 Jobs queue + worker convert — `server` `rust-core`
**Hướng làm:** Claim job `FOR UPDATE SKIP LOCKED`, `idempotency_key`, checkpoint per-step. Worker convert gọi `fileconv-core` trong process/`spawn_blocking` cách ly + timeout + kill; PDF 3-tier/OCR như hiện tại → Markdown → job index.
**Việc cần làm:**
- [ ] Queue claim/retry/attempts · [ ] Worker convert + sandbox + timeout
- [ ] Trạng thái từng bước ghi PG (UI đọc realtime)
**DoD:** kill worker giữa chừng → job resume đúng checkpoint; file hỏng → `failed` + lỗi đọc được, không treo queue.

### 2.7 State machine + reconciliation — `server`
**Hướng làm:** Trạng thái `uploaded→converting→converted→indexing→indexed|failed`, PG là nguồn sự thật, mọi ghi Qdrant/MinIO idempotent (key theo document version + batch). Delete = tombstone PG trước → job xóa Qdrant/MinIO. Reconciliation job định kỳ quét lệch 3 hệ (orphan file, stale vector) và sửa.
**Việc cần làm:**
- [ ] Transition hợp lệ + reject transition sai · [ ] Tombstone + job xóa
- [ ] Reconciliation job + test tạo lệch giả → tự sửa
**DoD:** citation/preview không bao giờ trả tài liệu tombstone; reconciliation sửa được lệch nhân tạo cả 3 chiều.

### 2.8 Quota atomic — `server`
**Hướng làm:** MỘT cơ chế reserve→finalize/refund trong PG (bảng `quota_reservations`) — KHÔNG check-then-act. Semaphore concurrent job theo org/user. 429 + quota headers.
**Việc cần làm:**
- [ ] Reserve/finalize/refund transaction · [ ] Tích hợp vào 2.5 (upload GB) + đếm LLM token thật từ usage (dùng ở 3.4)
- [ ] Test 2 request đồng thời sát trần quota
**DoD:** test đồng thời: tổng không vượt trần; refund khi job fail; vượt quota → 429.

### 2.9 Integration test M2 — `qa`
**Hướng làm:** Bộ test cấp hệ thống chạy trên compose: upload thật → convert → chunk trong PG; kill worker → resume; upload sample độc từ 1.8; quota concurrent (phối hợp 2.8).
**Việc cần làm:**
- [ ] Harness chạy test trên compose (CI được càng tốt) · [ ] Case cho từng DoD của 2.5-2.8
**DoD:** suite = chính là gate M2, chạy pass toàn bộ.

## Phase 3 — RAG pipeline (→ M3)

### 3.1 Embedding queue GLM — `rust-core`
**Hướng làm:** Queue riêng cho embed: batch scheduler, backpressure (bounded queue — ingest chậm lại thay vì OOM), retry policy theo rate limit GLM, **index signature** pin model+dimension+version (pattern desktop có sẵn — đổi model chỉ cần reindex).
**Việc cần làm:**
- [ ] Queue + batch + retry · [ ] Index signature vào PG
- [ ] Metric: chunk/s, token đã dùng (nối 2.8)
**DoD:** ingest 300-500 chunk golden-set ổn định; ngắt mạng giả → retry rồi tiếp tục đúng batch.

### 3.2 Index step — `server`
**Hướng làm:** Job index: chunk heading-path (từ `crates/knowledge`) → gửi 3.1 → upsert Qdrant (2.4) + insert `chunks` FTS, checkpoint per-batch (resume giữa chừng không tạo duplicate — idempotency theo chunk id).
**Việc cần làm:**
- [ ] Job type index + checkpoint · [ ] Test resume không duplicate vector/row
**DoD:** tài liệu golden-set index đủ 2 đường (Qdrant + FTS); kill giữa batch → resume sạch.

### 3.3 Hybrid search + rerank — `rust-core`
**Hướng làm:** Embed query → Qdrant top-k (filter org + collection ACL qua 2.4) ∥ PG FTS (`unaccent`+`simple`) → merge/rerank công thức RRF/hybrid từ `crates/knowledge` (đã khoá hành vi ở 1.3).
**Việc cần làm:**
- [ ] Endpoint search nội bộ trả chunk + score + nguồn · [ ] Test rank khớp kỳ vọng trên golden-set nhỏ
**DoD:** kết quả hybrid ≥ từng đường đơn lẻ trên golden-set (đo bằng 3.5).

### 3.4 Q&A endpoint SSE + citation — `server`
**Hướng làm:** Prompt kèm nguồn từ 3.3 → GLM stream SSE → answer + citation (doc, heading path, link). Citation fetch **re-check ACL + trạng thái tài liệu** (tombstone không bao giờ trả). LLM lỗi/timeout → fallback trả trích đoạn top chunk. Đếm token thật vào quota (2.8).
**Việc cần làm:**
- [ ] SSE endpoint + stream parse · [ ] Citation re-check · [ ] Fallback + test giả lập LLM chết
**DoD:** hỏi trên golden-set → answer stream + citation click ra đúng chunk; tắt GLM → vẫn trả trích đoạn, không 500.

### 3.5 Eval harness recall — `qa`
**Hướng làm:** Harness chạy toàn bộ câu hỏi golden-set qua 3.3/3.4, đo recall@k + citation đúng; xuất report so sánh được giữa các lần chạy (đổi model/công thức rerank → chạy lại).
**Việc cần làm:**
- [ ] Runner + report markdown · [ ] Lưu kết quả từng run có nhãn config
**DoD:** report recall đạt ngưỡng chốt ở M1 (**gate M3**).

## Phase 4 — POC UI & Demo (→ 🎯 POC)

### 4.1 UI Login + route guard — `frontend`
**Hướng làm:** Form login → lưu token (memory + refresh), guard route chưa auth → redirect. Interceptor 401 → refresh → retry 1 lần.
**Việc cần làm:** [ ] Form + validate · [ ] Guard + interceptor · [ ] Trạng thái lỗi rõ (sai pass / mạng)
**DoD:** login/logout/refresh chạy với 2.3; F5 không văng session khi refresh token còn hạn.

### 4.2 UI Thư viện — `frontend`
**Hướng làm:** Upload (đúng format POC, message từ chối `.doc` thân thiện), danh sách tài liệu + trạng thái ingest realtime (poll/SSE từ 2.6), preview Markdown (SafeMarkdown), xóa (tombstone). Pattern LibraryView desktop.
**Việc cần làm:** [ ] Upload + progress · [ ] List + badge trạng thái state machine · [ ] Preview + xóa
**DoD:** upload → nhìn thấy trạng thái chuyển `converting→indexed` → preview được MD; file bị chặn hiện lý do.

### 4.3 UI Chat Q&A citation — `frontend`
**Hướng làm:** Chat stream SSE từ 3.4, render markdown answer, citation là chip/link → mở panel preview đúng tài liệu + heading. Hiện trạng thái fallback ("LLM lỗi — trích đoạn nguồn") trung thực.
**Việc cần làm:** [ ] SSE client + render tăng dần · [ ] Citation chip → preview anchor · [ ] Trạng thái lỗi/fallback
**DoD:** hỏi thật trên UI → answer stream + click citation mở đúng chỗ; không giả tiến trình.

### 4.4 Playwright E2E smoke — `qa`
**Hướng làm:** Kịch bản: login → upload 1 pdf + 1 docx → chờ `indexed` → hỏi → assert citation link đúng tài liệu. CI: compose + embedding hash-local fallback (không cần GLM key trong CI).
**Việc cần làm:** [ ] Setup Playwright + fixture · [ ] Kịch bản smoke · [ ] Wire vào CI
**DoD:** suite xanh trên CI 3 lần liên tiếp (không flaky).

### 4.5 Demo POC tổng duyệt — cả team
**Hướng làm:** Ingest bộ tài liệu demo thật (chọn scan vừa phải — OCR 1-5s/trang), viết kịch bản demo, tổng duyệt, fix lỗi phát sinh. Checkpoint quyết định: nếu chưa qua → họp cắt Phase 5 hay lùi deadline (KHÔNG im lặng trượt).
**Việc cần làm:**
- [ ] Chọn + chuẩn bị bộ tài liệu demo (đủ mỗi định dạng POC 1 file, có 1 pdf scan)
- [ ] Viết kịch bản demo từng bước (ai bấm gì, hỏi câu gì, kỳ vọng thấy gì)
- [ ] Chạy tổng duyệt 2 lần, ghi lỗi phát sinh thành issue hotfix
- [ ] Họp checkpoint ngày 32: qua gate / cắt Phase 5 / lùi deadline
**DoD (= GATE POC):** demo end-to-end single-org qua UI: upload 1 tài liệu mỗi định dạng POC → hỏi đáp citation đúng.

## Phase 5 — Hoàn thiện SPA + Intelligence rút gọn (→ M5)

### 5.1 UI Admin member/role/usage — `frontend`
**Hướng làm:** Trang admin: danh sách member + role (đọc/ghi qua API RBAC sẵn schema), usage counters từ 2.8. Chỉ owner/admin thấy (guard perm).
**Việc cần làm:**
- [ ] API endpoint list/update membership (phối hợp M2 nếu 6.1 chưa tới — chỉ cần đọc/ghi role cơ bản)
- [ ] Trang member: bảng + đổi role (dropdown, confirm)
- [ ] Trang usage: đọc `usage_counters` (upload GB, LLM token, số job)
- [ ] Guard route theo perm `member.manage`/`audit.view`
**DoD:** thêm/đổi role member được; usage hiển thị đúng số thật; viewer không vào được trang.

### 5.2 Collection management + reindex UI — `frontend`
**Hướng làm:** CRUD collection + visibility (private/org/groups), gán tài liệu, nút reindex (tạo job index lại), hiển thị lỗi ingest chi tiết + retry.
**Việc cần làm:**
- [ ] CRUD collection + chọn visibility
- [ ] Gán/bỏ tài liệu vào collection (từ Thư viện 4.2)
- [ ] Nút reindex → tạo job index (đi qua queue 2.6, không đường tắt)
- [ ] Panel lỗi ingest: message từ `jobs.attempts/error` + nút retry
**DoD:** đổi visibility có hiệu lực ngay ở Q&A; reindex chạy như job thường (resume được).

### 5.3 Intelligence rút gọn: tóm tắt + PII — `rust-core`
**Hướng làm:** Port 2 tính năng từ `intelligence.rs`: tóm tắt tài liệu (GLM, cap ký tự như MCP `summarize`) + PII detect/redaction cơ bản. Còn lại (BA/PM handoff, quality, versions/diff) ở Backlog — KHÔNG kéo vào.
**Việc cần làm:**
- [ ] Khoanh vùng code tóm tắt + PII trong `intelligence.rs` (phần thuần đã sang `crates/knowledge` ở 2.1 chưa? — nếu chưa, chỉ port logic, không kéo dependency Tauri)
- [ ] Endpoint summarize (cap ký tự, token vào quota 2.8)
- [ ] Endpoint PII detect + redaction preview
- [ ] UI entry trong DocView/Thư viện + test từng endpoint
**DoD:** 2 tính năng chạy trên tài liệu đã index, có endpoint + UI entry + test; token tính vào quota.

### 5.4 Mở rộng Playwright E2E — `qa`
**Hướng làm:** Thêm kịch bản admin (đổi role → quyền đổi), collection (visibility → kết quả Q&A đổi), tóm tắt/PII.
**Việc cần làm:**
- [ ] Kịch bản: admin đổi role viewer→editor → quyền upload đổi ngay
- [ ] Kịch bản: đổi visibility collection → user ngoài không còn thấy trong Q&A
- [ ] Kịch bản: tóm tắt + PII trả kết quả trên tài liệu mẫu
- [ ] Wire vào CI cùng suite 4.4
**DoD:** suite mở rộng xanh trên CI (**gate M5** cùng demo 5.1-5.3).

## Phase 6 — RBAC & multi-org (→ M6)

### 6.1 RBAC role/ACL mức 2 đầy đủ — `server` `security`
**Hướng làm:** Hoàn thiện guard `require(perm)` phủ MỌI route (rà từng route một, làm checklist), ACL collection (bảng `collection_access`) enforce ở repo layer 2.2, permission string đúng danh sách design doc.
**Việc cần làm:**
- [ ] Lập bảng route × permission (mọi route hiện có, kể cả healthz/internal)
- [ ] Gắn guard từng route theo bảng; route thiếu perm phù hợp → thêm permission mới có chủ đích
- [ ] Enforce `collection_access` trong repo layer (2.2) cho search/citation/preview/download
- [ ] Cache permission TTL ngắn + invalidate khi đổi role/ACL
**DoD:** bảng route × permission được review; không route nào thiếu guard; ACL đổi có hiệu lực ngay (cache TTL ngắn).

### 6.2 Rate limit 2 tầng + quota dashboard — `server`
**Hướng làm:** tower_governor per-user (per-IP khi chưa auth), auth endpoints chặt hơn; dashboard usage per-org cho admin (mở rộng 5.1); in-memory 1 node (Redis chỉ khi multi-replica — chưa cần).
**Việc cần làm:**
- [ ] Layer tower_governor: config per-user (key = user id) + per-IP cho route chưa auth
- [ ] Rate chặt riêng cho login/refresh (chống brute-force)
- [ ] Response 429 + headers (limit/remaining/reset)
- [ ] Dashboard per-org (mở rộng trang usage 5.1): quota trần vs đã dùng
**DoD:** spam request → 429 có header; dashboard khớp usage_counters.

### 6.3 Denial test suite — `qa` `security`
**Hướng làm:** Suite phủ đủ 5 đường: Qdrant search, PG FTS, citation fetch, preview, download — user ngoài collection/org không nhận nội dung qua bất kỳ đường nào. Thêm case: token org A gọi resource org B, user bị thu hồi quyền giữa session.
**Việc cần làm:**
- [ ] Fixture 2 org + 3 user (owner org A, viewer org A ngoài collection, user org B)
- [ ] Case từng đường × từng user (ma trận 5 đường × 3 user)
- [ ] Case thu hồi quyền giữa session (cache TTL phải hết hiệu lực đúng)
- [ ] Case tài liệu tombstone không trả qua bất kỳ đường nào
- [ ] Wire vào CI, chạy mọi PR từ đây
**DoD:** suite pass = **gate M6**; chạy trong CI từ đây về sau.

### 6.4 Rollout multi-org — cả team
**Hướng làm:** Tạo org thứ 2 thật, ingest dữ liệu riêng, demo cách ly (search/Q&A/quota độc lập), rà quota per-org.
**Việc cần làm:**
- [ ] Flow tạo org mới + owner đầu tiên (script/API admin)
- [ ] Ingest bộ dữ liệu riêng cho org 2
- [ ] Chạy denial suite (6.3) trên môi trường 2 org thật
- [ ] Demo kịch bản: cùng câu hỏi, 2 org ra kết quả từ dữ liệu riêng
**DoD:** demo 2 org song song; không rò dữ liệu chéo (denial suite chạy trên môi trường 2 org).

## Phase 7 — Hardening & rollout (→ M7)

### 7.1 Backup/recovery drill — `infra`
**Hướng làm:** PG backup + restore; Qdrant snapshot/restore (bổ sung phần cắt từ 1.10); drill: xoá Qdrant → rebuild index từ PG (chunks là nguồn sự thật); đo thời gian phục hồi.
**Việc cần làm:**
- [ ] Script PG backup (pg_dump/basebackup) + restore thử sang instance sạch
- [ ] Qdrant snapshot + restore thử
- [ ] Drill rebuild: xoá collection Qdrant → chạy reindex từ PG chunks → so kết quả Q&A trước/sau
- [ ] Đo + ghi thời gian từng bước → runbook `docs/`
**DoD:** drill thành công có ghi lại từng bước + thời gian; tài liệu runbook.

### 7.2 Security checklist nội bộ — `security`
**Hướng làm:** Rà theo threat model 1.8 + OWASP ASVS mức cơ bản: authz từng route (từ 6.1), audit_log ghi đủ hành vi nhạy cảm, secret không vào log, TLS/headers.
**Việc cần làm:**
- [ ] Đối chiếu bảng route × permission (6.1) lần cuối
- [ ] Rà audit_log: login/fail, đổi role/ACL, xóa tài liệu, export — đủ chưa
- [ ] Grep log output: không secret/token/PII
- [ ] Security headers + TLS config reverse proxy
- [ ] Mục chưa đóng → issue `backlog` có người phụ trách
**DoD:** checklist đóng từng mục có bằng chứng; mục chưa đóng → issue backlog có chủ.

### 7.3 Trang hướng dẫn + onboarding — `frontend`
**Hướng làm:** Trang help trong app (markdown render sẵn có): hướng dẫn upload/format hỗ trợ/hỏi đáp/citation; onboarding lần đầu đăng nhập.
**Việc cần làm:**
- [ ] Nội dung help: format hỗ trợ (+ lý do từ chối .doc), flow upload→hỏi đáp, đọc citation
- [ ] Onboarding lần đầu login (checklist/tour ngắn, bỏ qua được)
- [ ] Link help từ các điểm hay vướng (upload bị chặn, Q&A fallback)
**DoD:** user mới tự dùng được không cần hỏi; nội dung khớp tính năng thật (không hứa tính năng backlog).

### 7.4 Auth interface OIDC-ready — `server`
**Hướng làm:** Refactor module auth (2.3) ra trait/interface để cắm OIDC provider sau mà không sửa call-site; document flow tích hợp IdP. KHÔNG tích hợp IdP thật (Backlog).
**Việc cần làm:**
- [ ] Trait auth provider (verify credential → identity) + impl JWT hiện tại
- [ ] Điểm nối OIDC (callback route placeholder, mapping identity→user/org) — chỉ khung
- [ ] Doc flow tích hợp IdP cho người làm sau
- [ ] Regression: toàn bộ test auth 2.3 pass nguyên trạng
**DoD:** JWT flow hiện tại chạy nguyên trạng qua interface mới; doc tích hợp được lead duyệt.

---

## Backlog sau 45 ngày (tạo issue label `backlog`, không milestone)

BA/PM handoff + quality + versions/diff/merge · agent-driven browser test (Playwright MCP) ·
SSO/OIDC tích hợp IdP thật · pentest bên ngoài · chuyển embedding GLM → vLLM GPU (reindex) ·
Qdrant benchmark đầy đủ + snapshot tự động · audio upload (check license PhoWhisper trước).
