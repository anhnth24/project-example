# Markhand Web — Phase plan (POC deadline 40-45 ngày)

> Thiết kế hệ thống: [`../docs/web-architecture.md`](../docs/web-architecture.md).
> Chẻ task chi tiết (hướng làm/tiêu chí/cạm bẫy): [`260713-markhand-web-task-breakdown.md`](260713-markhand-web-task-breakdown.md).
> Nguồn: brainstorm APPROVED 2026-07-13 (Codex review 8 finding) + review đối kháng nội bộ (10 finding).
> Nhân sự: 3-4 member (đa số chưa vững Rust) + lead + AI agent. Ngày = ngày làm việc.

## Nguyên tắc (không thương lượng)

1. **Vertical slice**: single-org POC end-to-end trước, multi-org sau. Schema có `org_id`
   mọi bảng từ đầu → lên multi-org không cần migration phá.
2. **Không generic hoá storage sớm**: tách phần thuần (chunk/rank/citation) trước,
   adapter PG/Qdrant viết theo nhu cầu server.
3. **Gate là nhị phân**: chưa qua gate = chưa xong, không "xong 80%".
4. **Tenant-scoped repository + quota atomic + upload hardening làm ngay ở Phase 2**,
   không dồn về cuối (finding review).

## Tổng tiến độ

```
Ngày:    1        10        20   25        33            45
Phase 1 ████████████
Phase 2          ████████████████
Phase 3                    ████████████
Phase 4                         █████████████████
                 ▲M1       ▲M2  ▲M3        🎯 POC (ngày 40-45)
Phase 5/6/7 — sau POC
```

Phase 2/3/4 gối đầu nhau có chủ đích (Phase 3 bắt đầu khi 2.1+2.4 xong; Phase 4 UI làm
trên mock từ ngày 28) — không chờ phase trước kết thúc hoàn toàn.

---

## Phase 1 — Nền tảng & Spike (ngày 1-10) → Milestone M1

**Tổng quan:** dựng hạ tầng dev + khung code server/web + chuẩn code + dữ liệu đánh giá;
chạy gate embedding/Qdrant; viết test khoá hành vi desktop trước khi tách core.
Mọi người có việc từ ngày 1.

| # | Subtask | Ai | Ngày |
|---|---|---|---|
| 1.1 | Docker-compose PG/Qdrant/MinIO + healthcheck (`bench/web-spike/`) | Member 1 | 1-2 |
| 1.2 | Viết `docs/web-code-standards.md` (M2 phần server, M3 phần web) | M2+M3, lead duyệt | 1-3 |
| 1.3 | Test khoá hành vi `knowledge.rs`/`vector_index.rs` (trước khi tách) | Lead + AI | 1-5 |
| 1.4 | Skeleton `web/`: Vite, 4 trang placeholder, api client, Zustand + LumiBase token + lint | Member 3 | 1-7 |
| 1.5 | Golden-set tiếng Việt: 500-1K chunk + 50-100 cặp Q/A có nguồn | Member 1 | 3-7 |
| 1.6 | Skeleton `crates/server`: axum, config env, error type, `/healthz` | Member 2 + lead pair | 3-7 |
| 1.7 | PG schema migration (sqlx) + seed dev | Member 2 | 5-10 |
| 1.8 | Threat model upload (input cho 2.5) | Member 1 | 5-6 |
| 1.9 | Eval GLM embedding trên golden-set: recall@1/5/10 vs hash-local + chi phí/latency | Member 1 | 7-10 |
| 1.10 | Benchmark Qdrant multi-tenant (filter P95, RAM sau quantization, snapshot/restore) | Member 1 | 7-12 (tràn sang P2 được) |

**🏁 Gate M1 (ngày 10):** 3 service compose healthy · skeleton server+web build, CI pass ·
migration chạy sạch · **recall GLM embedding đạt ngưỡng thống nhất** (không đạt → đổi
provider embedding ngay tại đây, chưa mất công ingest).

## Phase 2 — Tách core & Integration (ngày 11-25) → Milestone M2

**Tổng quan:** tách `crates/knowledge`; server nối PG/Qdrant/MinIO qua tenant-scoped
repository; auth JWT; upload hardening; worker convert; state machine; quota atomic.
**Critical path toàn dự án** (phần lead + AI).

| # | Subtask | Ai | Ngày |
|---|---|---|---|
| 2.1 | Tách `crates/knowledge` — desktop hành vi không đổi (gate: cargo test + desktop chạy đúng) | Lead + AI | 8-15 |
| 2.2 | Tenant-scoped repository (`OrgContext`) — nền MỌI data-access, làm một lần | Lead + AI | 11-14 |
| 2.3 | Auth JWT + refresh token (module tách riêng, pluggable OIDC sau) | Lead khung, M2 endpoint | 12-16 |
| 2.4 | Qdrant collection config + adapter (dimension theo 1.9, config theo 1.10; adapter từ chối search thiếu org_id) | Member 2 | 13-15 |
| 2.5 | Upload API: MinIO, MIME sniff magic-byte, size limit, zip-bomb, quarantine, từ chối `.doc` | Member 2 (chuỗi task nhỏ) | 14-20 |
| 2.6 | Jobs queue (`FOR UPDATE SKIP LOCKED`, idempotency_key) + worker convert (spawn_blocking/process riêng, sandbox + timeout + kill) | Lead + AI | 15-20 |
| 2.7 | Document state machine (`uploaded→…→indexed|failed`) + tombstone + reconciliation | Lead + AI | 18-24 |
| 2.8 | Quota reserve→finalize/refund atomic (MỘT cơ chế, không bản tạm check-then-act) | Lead + AI | 20-23 |
| 2.9 | Integration test: kill worker→resume checkpoint, quota concurrent, upload adversarial (sample từ 1.8) | Member 1 | 18-25 |

**🏁 Gate M2 (ngày 25):** upload → convert → chunk vào PG chạy thật · job resume đúng
checkpoint · upload độc hại bị chặn trước converter · 2 request đồng thời không vượt quota.

## Phase 3 — RAG pipeline (ngày 20-33, gối đầu Phase 2) → Milestone M3

**Tổng quan:** embedding queue GLM + index Qdrant/FTS + hybrid Q&A citation SSE.
Bắt đầu khi 2.1 + 2.4 xong, không chờ hết Phase 2.

| # | Subtask | Ai | Ngày |
|---|---|---|---|
| 3.1 | Embedding queue GLM API: batch scheduler, backpressure, retry, index signature (pin model+dim+version) | Lead + AI | 20-26 |
| 3.2 | Index step: chunk heading-path → Qdrant upsert + insert `chunks` FTS, checkpoint per-batch | Member 2 + lead | 24-28 |
| 3.3 | Hybrid search + rerank (công thức từ `crates/knowledge`; filter org + collection ACL) | Lead + AI | 26-30 |
| 3.4 | Q&A endpoint: GLM SSE stream + citation re-check ACL/trạng thái doc + fallback trích đoạn khi LLM lỗi | Lead + AI | 28-32 |
| 3.5 | Eval harness recall trên golden-set — chạy lại được mỗi lần đổi model/rerank | Member 1 | 30-33 |

**🏁 Gate M3 (ngày 33):** API hỏi đáp trả citation đúng trên golden-set, recall đạt
ngưỡng đã chốt ở M1.

## Phase 4 — POC UI & Demo (ngày 28-45, gối đầu Phase 3) → 🎯 POC

**Tổng quan:** UI tối thiểu đủ demo POC + Playwright smoke + tổng duyệt. Member 3 làm UI
trên API mock từ ngày 28, nối API thật dần khi Phase 3 xong từng phần.

| # | Subtask | Ai | Ngày |
|---|---|---|---|
| 4.1 | Login + route guard | Member 3 | 28-31 |
| 4.2 | Thư viện: upload (format POC, từ chối `.doc` với message), trạng thái ingest realtime, preview markdown, xóa | Member 3 | 30-38 |
| 4.3 | Chat Q&A citation: SSE stream, click citation mở đúng tài liệu/heading | Member 3 | 33-40 |
| 4.4 | Playwright E2E smoke: login → upload → theo dõi index → hỏi → verify citation (CI: compose + hash-local fallback) | Member 1 | 36-42 |
| 4.5 | Ingest bộ tài liệu demo thật + tổng duyệt kịch bản demo + fix lỗi | Cả team | 40-44 |

**🎯 GATE POC (ngày 40-45):** demo end-to-end single-org **qua UI**: upload 1 tài liệu
mỗi định dạng POC → hỏi đáp ra citation đúng. Phase 2/3/4 không được báo "xong" riêng lẻ
khi chưa qua gate chung này.

---

## Sau POC (ngoài deadline 45 ngày)

| Phase | Tổng quan | Subtask chính | Gate | Ước lượng |
|---|---|---|---|---|
| **5 — Web SPA hoàn thiện + Intelligence** | Từ POC UI lên MVP đầy đủ + port intelligence | Admin (member/role/usage) · collection management + reindex · versions/diff · port tóm tắt, quality, PII/redaction, BA/PM handoff từ `intelligence.rs` · agent-driven browser test (AI agent + Playwright MCP exploratory trên staging) | Mỗi tính năng có test + demo; E2E suite xanh | ~3-4 tuần |
| **6 — RBAC & multi-org** | Từ single-org POC lên multi-org production | Role/ACL mức 2 đầy đủ · rate limit 2 tầng (tower_governor + quota dashboard per-org) · **denial test suite** phủ Qdrant search/PG FTS/citation/preview/download · rollout multi-org | Denial suite pass: user ngoài collection không nhận nội dung qua bất kỳ đường nào; vượt quota → 429 | ~2-3 tuần |
| **7 — Hardening & rollout** | Sẵn sàng vận hành thật | Audit review + pentest checklist · SSO/OIDC · trang hướng dẫn + onboarding · backup/recovery drill (PG backup, Qdrant snapshot, rebuild index từ PG) · kiểm tra license PhoWhisper nếu bật audio | Recovery drill thành công; pentest checklist đóng | ~2 tuần |

## Điều kiện để lịch 40-45 ngày đứng vững

1. [Inference] **Lead + AI agent full-time trên critical path** (2.1→2.7→3.1→3.4). Tách
   5.200 dòng trong ~8 ngày là mức căng; lead chỉ part-time → POC trượt ~55-60 ngày.
2. **GLM endpoint + API key có trước ngày 5** — không thì 1.9 trượt, gate M1 mất nghĩa.
3. Máy chạy docker-compose sẵn từ ngày 1.
4. Estimate là ngày làm việc, chưa buffer ốm/việc đột xuất — range 40-45 chính là buffer.

## Rủi ro chính (từ brainstorm gốc, gắn vào phase)

| Rủi ro | Phase chặn |
|---|---|
| [Unverified] Chất lượng embedding tiếng Việt qua GLM API | Gate M1 (1.9) — đổi provider tại đây nếu fail |
| Refactor 5.200 dòng gắn Tauri+SQLite | 2.1 — critical path, test khoá 1.3 làm lưới |
| Consistency PG/Qdrant/MinIO — vùng dễ bug nhất | 2.7 + test 2.9 |
| OCR cổ chai CPU (1-5s/trang scan) | 3.2/4.5 — demo chọn scan vừa phải, scale worker sau POC |
| PhoWhisper license chưa rõ | Phase 7 (audio chưa vào POC) |

## Quyết định đã chốt với user (2026-07-13)

1. Quy ước code server+web: file riêng `docs/web-code-standards.md` (task 1.2).
2. Upload POC: **docx, xlsx, pdf (text + scan), csv, md, txt, ảnh OCR**; từ chối `.doc`
   binary cũ với thông báo hướng dẫn convert sang docx; chưa nhận audio.
3. Nhân sự: 3-4 member đa số chưa vững Rust + lead + AI agent — Rust nặng dồn lead/AI.
4. Embedding POC: **GLM embedding API** (chưa có GPU); vLLM local khi có GPU — index
   signature pin model nên chuyển đổi chỉ cần reindex.
5. Deadline: **POC trong 40-45 ngày**.

## Câu hỏi mở còn lại

1. GLM endpoint + API key — chặn 1.9, cần trước ngày 5.
2. Máy chạy PG/Qdrant/MinIO dev/staging — chặn 1.1.
3. Số SLA cụ thể — chốt sau gate M1 (kết quả 1.9/1.10).
