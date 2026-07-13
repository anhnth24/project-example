# Markhand Web — Phase plan (TOÀN BỘ 7 phase trong 45 ngày)

> Thiết kế hệ thống: [`../../docs/web-architecture.md`](../../docs/web-architecture.md).
> Chẻ task chi tiết (hướng làm/tiêu chí/cạm bẫy): [`260713-task-breakdown.md`](260713-task-breakdown.md).
> Nguồn: brainstorm APPROVED 2026-07-13 (Codex review 8 finding) + review đối kháng nội bộ (10 finding).
> Nhân sự: 3-4 member (đa số chưa vững Rust) + lead + AI agent. Ngày = ngày làm việc.
> **Deadline user chốt: cả Phase 1-7 trong 40-45 ngày** → POC kéo lên ngày 30-32,
> Phase 5-7 rút gọn scope (phần cắt xuống mục Backlog cuối file).

## Nguyên tắc (không thương lượng)

1. **Vertical slice**: single-org POC end-to-end trước, multi-org sau. Schema có `org_id`
   mọi bảng từ đầu → lên multi-org không cần migration phá.
2. **Không generic hoá storage sớm**: tách phần thuần (chunk/rank/citation) trước,
   adapter PG/Qdrant viết theo nhu cầu server.
3. **Gate là nhị phân**: chưa qua gate = chưa xong, không "xong 80%".
4. **Tenant-scoped repository + quota atomic + upload hardening làm ngay ở Phase 2**,
   không dồn về cuối (finding review).

## Tổng tiến độ 45 ngày

```
Ngày:    1      8       16      19      26      32      38   42   45
Phase 1 ████████
Phase 2       █████████████
Phase 3                ███████████
Phase 4                    █████████████
Phase 5                              █████████
Phase 6                                ███████████
Phase 7                                        ██████
               ▲M1      ▲M2     ▲M3    🎯POC   ▲M5     ▲M6  ▲M7(45)
```

Gối đầu có chủ đích: Phase 3 bắt đầu khi 2.1+2.4 xong; Phase 4 UI làm trên mock từ
ngày 20; Phase 6 khởi động từ ngày 32 vì tenant-repo/quota đã có sẵn từ Phase 2.

---

## Phase 1 — Nền tảng & Spike (ngày 1-8) → M1

**Tổng quan:** hạ tầng dev + khung code server/web + chuẩn code + dữ liệu đánh giá rút gọn;
gate embedding/Qdrant; test khoá hành vi desktop trước khi tách core.

| # | Subtask | Ai | Ngày |
|---|---|---|---|
| 1.1 | Docker-compose PG/Qdrant/MinIO + healthcheck (`bench/web-spike/`) | Member 1 | 1-2 |
| 1.2 | `docs/web-code-standards.md` (M2 phần server, M3 phần web) | M2+M3, lead duyệt | 1-3 |
| 1.3 | Test khoá hành vi `knowledge.rs`/`vector_index.rs` | Lead + AI | 1-4 |
| 1.4 | Skeleton `web/`: Vite, 4 trang, api client, Zustand + LumiBase token + lint | Member 3 | 1-6 |
| 1.5 | Golden-set tiếng Việt **rút gọn**: 300-500 chunk + 30-50 cặp Q/A | Member 1 | 3-6 |
| 1.6 | Skeleton `crates/server`: axum, config env, error type, `/healthz` | Member 2 + lead pair | 3-6 |
| 1.7 | PG schema migration (sqlx) + seed dev | Member 2 | 4-8 |
| 1.8 | Threat model upload (input cho 2.5) | Member 1 | 4-5 |
| 1.9 | Eval GLM embedding: recall@1/5/10 vs hash-local + chi phí/latency | Member 1 | 6-8 |
| 1.10 | Benchmark Qdrant **lite**: filter P95 + RAM quantization (1M vector; snapshot/restore đẩy sang 7.1) | Member 1 | 6-9 |

**🏁 M1 (ngày 8):** compose healthy · skeleton build + CI pass · migration sạch ·
**recall GLM đạt ngưỡng** (fail → đổi provider ngay, chưa mất gì).

## Phase 2 — Tách core & Integration (ngày 7-19) → M2

**Tổng quan:** tách `crates/knowledge`; tenant-scoped repository; auth JWT; upload
hardening; worker convert; state machine; quota atomic. **Critical path toàn dự án.**

| # | Subtask | Ai | Ngày |
|---|---|---|---|
| 2.1 | Tách `crates/knowledge` — desktop hành vi không đổi | Lead + AI | 5-13 |
| 2.2 | Tenant-scoped repository (`OrgContext`) — nền MỌI data-access | Lead + AI | 9-12 |
| 2.3 | Auth JWT + refresh (module tách riêng, pluggable OIDC) | Lead khung, M2 endpoint | 10-13 |
| 2.4 | Qdrant collection config + adapter (theo 1.9/1.10; từ chối search thiếu org_id) | Member 2 | 10-12 |
| 2.5 | Upload API: MinIO, MIME sniff, size limit, zip-bomb, quarantine, từ chối `.doc` | Member 2 (chuỗi task nhỏ) | 11-16 |
| 2.6 | Jobs queue (`SKIP LOCKED`, idempotency_key) + worker convert (sandbox + timeout + kill) | Lead + AI | 12-16 |
| 2.7 | State machine (`uploaded→…→indexed|failed`) + tombstone + reconciliation | Lead + AI | 14-19 |
| 2.8 | Quota reserve→finalize/refund atomic (MỘT cơ chế, không bản tạm) | Lead + AI | 16-18 |
| 2.9 | Integration test: kill worker→resume, quota concurrent, upload adversarial | Member 1 | 14-19 |

**🏁 M2 (ngày 19):** upload → convert → chunk vào PG chạy thật · resume đúng checkpoint ·
upload độc bị chặn · 2 request đồng thời không vượt quota.

## Phase 3 — RAG pipeline (ngày 16-26, gối đầu P2) → M3

| # | Subtask | Ai | Ngày |
|---|---|---|---|
| 3.1 | Embedding queue GLM: batch, backpressure, retry, index signature | Lead + AI | 16-20 |
| 3.2 | Index step: chunk → Qdrant upsert + `chunks` FTS, checkpoint per-batch | Member 2 + lead | 18-22 |
| 3.3 | Hybrid search + rerank (từ `crates/knowledge`; filter org + ACL) | Lead + AI | 20-24 |
| 3.4 | Q&A endpoint: GLM SSE + citation re-check + fallback trích đoạn | Lead + AI | 22-26 |
| 3.5 | Eval harness recall golden-set (chạy lại được khi đổi model/rerank) | Member 1 | 24-26 |

**🏁 M3 (ngày 26):** API hỏi đáp trả citation đúng, recall đạt ngưỡng M1.

## Phase 4 — POC UI & Demo (ngày 20-32, gối đầu P3) → 🎯 POC

**Tổng quan:** UI tối thiểu demo POC. Member 3 làm trên API mock từ ngày 20, nối API
thật dần theo tiến độ Phase 3.

| # | Subtask | Ai | Ngày |
|---|---|---|---|
| 4.1 | Login + route guard | Member 3 | 20-22 |
| 4.2 | Thư viện: upload (format POC), trạng thái ingest realtime, preview MD, xóa | Member 3 | 22-27 |
| 4.3 | Chat Q&A citation: SSE, click citation mở đúng tài liệu/heading | Member 3 | 25-30 |
| 4.4 | Playwright E2E smoke: login→upload→index→hỏi→verify citation | Member 1 | 27-31 |
| 4.5 | Ingest tài liệu demo thật + tổng duyệt + fix | Cả team | 30-32 |

**🎯 GATE POC (ngày 30-32):** demo end-to-end single-org qua UI: upload 1 tài liệu mỗi
định dạng POC → hỏi đáp citation đúng. Phase 2/3/4 không báo "xong" riêng lẻ trước gate này.

## Phase 5 — Hoàn thiện SPA + Intelligence rút gọn (ngày 30-38) → M5

**Tổng quan:** từ POC UI lên bản dùng được hàng ngày + 2 tính năng intelligence giá trị
nhất. Phần cắt: xem Backlog.

| # | Subtask | Ai | Ngày |
|---|---|---|---|
| 5.1 | Admin: member/role/usage view (đọc từ bảng RBAC sẵn có) | Member 3 | 30-34 |
| 5.2 | Collection management + reindex + xử lý lỗi ingest trên UI | Member 3 | 33-37 |
| 5.3 | Intelligence rút gọn: **tóm tắt tài liệu + PII/redaction cơ bản** (port từ `intelligence.rs`) | Lead + AI | 30-36 |
| 5.4 | Mở rộng Playwright E2E phủ admin + collection + intelligence | Member 1 | 32-38 |

**🏁 M5 (ngày 38):** admin/collection/tóm tắt/PII demo được qua UI; E2E suite xanh.

## Phase 6 — RBAC & multi-org (ngày 32-42) → M6

**Tổng quan:** lên multi-org thật. Khởi động được sớm vì tenant-repo (2.2) + quota (2.8)
+ schema org_id đã có từ Phase 2.

| # | Subtask | Ai | Ngày |
|---|---|---|---|
| 6.1 | Role/ACL mức 2 đầy đủ: guard `require(perm)` mọi route, ACL collection | Lead + M2 | 32-38 |
| 6.2 | Rate limit 2 tầng: tower_governor per-user + quota dashboard per-org | M2 + lead | 35-40 |
| 6.3 | **Denial test suite**: Qdrant search / PG FTS / citation / preview / download | Member 1 | 36-42 |
| 6.4 | Rollout multi-org: tạo org thứ 2, demo cách ly dữ liệu 2 org | Cả team | 40-42 |

**🏁 M6 (ngày 42):** denial suite pass — user ngoài collection không nhận nội dung qua
bất kỳ đường nào; vượt quota → 429; demo 2 org cách ly.

## Phase 7 — Hardening & rollout (ngày 40-45) → M7

**Tổng quan:** đủ điều kiện vận hành nội bộ. SSO/OIDC chỉ làm interface pluggable
(đúng quyết định gốc: JWT cho POC, SSO tương lai).

| # | Subtask | Ai | Ngày |
|---|---|---|---|
| 7.1 | Backup/recovery drill: PG backup, Qdrant snapshot/restore, rebuild index từ PG | Member 1 | 40-43 |
| 7.2 | Security checklist nội bộ + rà audit_log (theo threat model 1.8) | Lead | 40-44 |
| 7.3 | Trang hướng dẫn sử dụng + onboarding trong app | Member 3 | 40-44 |
| 7.4 | Auth interface OIDC-ready (chưa tích hợp IdP thật) | Lead | 43-45 |

**🏁 M7 (ngày 45):** recovery drill thành công; checklist security đóng; help page live.

---

## Backlog sau 45 ngày (phần cắt để vừa deadline — KHÔNG âm thầm bỏ)

- Intelligence còn lại: **BA/PM handoff, quality report, versions/diff/merge**.
- **Agent-driven browser test** (AI agent + Playwright MCP exploratory trên staging).
- **SSO/OIDC tích hợp IdP thật** (interface đã sẵn từ 7.4).
- Pentest bên ngoài (7.2 mới là checklist nội bộ).
- Chuyển embedding GLM API → **vLLM GPU local** khi có GPU (reindex theo index signature).
- **Chế độ 100% offline** (Ollama/vLLM local cho cả chat + embedding): client sẵn trong
  `llm.rs`, chỉ đổi provider + reindex theo `embedding_signatures` — cho org không chấp
  nhận cloud, khớp định vị offline-first/PDPL của dự án.
- Qdrant benchmark đầy đủ phân bố org lệch + snapshot định kỳ tự động.
- Audio upload (kiểm tra license PhoWhisper trước).

## Điều kiện để lịch 45 ngày đứng vững — đọc kỹ

1. [Inference] **Đây là lịch không có slack.** Mọi estimate là ngày làm việc liền mạch,
   gối đầu dày đặc; 1 hạng mục critical path trượt 3 ngày là M7 vượt 45. Range 40-45 của
   POC cũ giờ đã dùng làm buffer cho Phase 5-7 rồi.
2. [Inference] **Lead + AI agent full-time** trên 2.1→2.7→3.1→3.4→5.3→6.1. Lead part-time
   → không giữ được lịch này, phải cắt thêm Phase 5 hoặc lùi deadline.
3. **GLM endpoint + API key trước ngày 5**; máy docker-compose sẵn ngày 1.
4. Member 2 (Rust junior) có lead/AI review nhanh trong ngày — PR treo 2 ngày là 2.5 vỡ lịch.
5. Nếu đến ngày 32 GATE POC chưa qua → **dừng lại quyết định**: cắt Phase 5 (giữ 6+7)
   hay lùi deadline — không im lặng trượt.

## Rủi ro chính (gắn vào phase)

| Rủi ro | Phase chặn |
|---|---|
| [Unverified] Chất lượng embedding tiếng Việt qua GLM API | Gate M1 (1.9) — đổi provider tại đây nếu fail |
| Refactor 5.200 dòng gắn Tauri+SQLite | 2.1 — critical path, test khoá 1.3 làm lưới |
| Consistency PG/Qdrant/MinIO — vùng dễ bug nhất | 2.7 + test 2.9 |
| OCR cổ chai CPU (1-5s/trang scan) | 3.2/4.5 — demo chọn scan vừa phải, scale worker sau |
| Lịch không slack (điều kiện mục trên) | Toàn bộ — checkpoint quyết định ở ngày 32 |

## Quyết định đã chốt với user (2026-07-13)

1. Quy ước code server+web: file riêng `docs/web-code-standards.md` (task 1.2).
2. Upload POC: **docx, xlsx, pdf (text + scan), csv, md, txt, ảnh OCR**; từ chối `.doc`
   với thông báo hướng dẫn convert sang docx; chưa nhận audio.
3. Nhân sự: 3-4 member đa số chưa vững Rust + lead + AI agent — Rust nặng dồn lead/AI.
4. Embedding POC: **GLM embedding API**; vLLM local khi có GPU (reindex theo index signature).
5. Deadline: **toàn bộ Phase 1-7 trong 40-45 ngày** (POC ngày 30-32; scope Phase 5-7 rút
   gọn theo mục Backlog).

## Câu hỏi mở còn lại

1. GLM endpoint + API key — chặn 1.9, cần trước ngày 5.
2. Máy chạy PG/Qdrant/MinIO dev/staging — chặn 1.1.
3. Số SLA cụ thể — chốt sau gate M1 (kết quả 1.9/1.10).
