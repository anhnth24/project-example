# Markhand Web — Phase plan (Track/Milestone + Gate)

> Thiết kế hệ thống: [`../docs/web-architecture.md`](../docs/web-architecture.md).
> Nguồn: brainstorm APPROVED 2026-07-13 (đã qua Codex review 8 finding) + review đối kháng
> nội bộ trên bản phasing này (10 finding — các sửa đổi chính ghi ở mục "Thay đổi so với draft").
> Mục tiêu file này: khung phase/track có gate đo được để **bước kế tiếp chia nhỏ task giao member**.

## Nguyên tắc giữ nguyên từ brainstorm gốc (không thương lượng)

1. **Vertical slice trước**: single-org POC chạy end-to-end rồi mới multi-org. Schema có
   `org_id` mọi bảng từ đầu nên bước lên multi-org không cần migration phá.
2. **Không generic hoá storage sớm**: tách phần thuần (chunk/rank/citation) trước,
   adapter PG/Qdrant viết theo nhu cầu server.
3. **Gate là nhị phân**: chưa qua gate = chưa xong, không "xong 80%".
4. **Hardening upload + tenant-scoped repository + quota atomic làm ngay ở Integration**,
   không dồn về phase cuối.

## Sơ đồ phụ thuộc

```
 Track A (Spike/GATE) ──────────┐
 Track B (Core reuse) ──────────┼──► M1 Integration ──► M2 RAG ──► GATE POC ──► M3a Web SPA ──► M4 RBAC/multi-org ──► M5 Hardening
 Track C (Nền tảng web) ────────┘         ▲                                      │
   (C-Qdrant config chờ gate A)           │                                      └─► M3b Intelligence (song song, không chặn M4)
 Track T (Test automation) ───── xuyên suốt từ M1 ─────────────────────────────►
```

A, B, C **chạy song song từ ngày 1**. M1 là điểm hội tụ đầu tiên (cần cả ba).

## Track song song (bắt đầu ngay)

| Track | Nội dung | Gate (nhị phân) | Rủi ro gắn kèm |
|---|---|---|---|
| **A — Spike & benchmark** (map Phase 0 gốc) | Benchmark Qdrant + PG FTS với phân bố org thật (payload filter latency, delete/update, snapshot/restore, RAM sau quantization); eval embedding golden-set tiếng Việt (bge-m3 vs e5); upload threat model; chốt số SLA | Số liệu đạt ngưỡng thống nhất → mới chốt Qdrant config + model embed + cho phép ingest hàng loạt | #1 embedding vi, #4 GPU chưa xác nhận |
| **B — Core reuse** (map Phase 1A gốc) | Tách `crates/knowledge` từ `app/src-tauri/{knowledge,vector_index,intelligence}.rs`: extract logic thuần chunk→embed→rank→citation; desktop giữ SQLite/HNSW | `cargo test` pass + desktop hành vi không đổi (test khoá hành vi viết TRƯỚC khi tách) | #3 refactor 5.200 dòng — track rủi ro cao nhất, bắt đầu sớm nhất |
| **C — Nền tảng web** | **C1a** PG schema + migration + docker-compose dev (PG/Qdrant/MinIO) — schema đã chốt trong design doc, KHÔNG chờ A. **C1b** Qdrant collection config — **chờ gate A**. **C2** skeleton `crates/server` (axum, layout module) + skeleton `web/` (Vite, routing) | Migration chạy sạch trên PG mới; skeleton build + CI pass | — |
| **C0 — Kickoff checklist** (task setup, KHÔNG phải phase) | Port token LumiBase từ `app/styles.css` sang `web/`; viết quy ước code server+web (phụ lục `docs/code-standards.md` hoặc file riêng — chờ chốt); ESLint/Prettier/CI cho `web/` | Checklist tick hết trong tuần đầu | — |

## Milestone tuần tự (điểm hội tụ)

| Milestone | Nội dung | Gate (nhị phân) | Rủi ro gắn kèm |
|---|---|---|---|
| **M1 — Integration** (map 1B gốc, phần pipeline) | Wire server ↔ knowledge ↔ PG/Qdrant/MinIO. **Tenant-scoped repository (`OrgContext`) xây MỘT LẦN tại đây** — mọi data-access sau đều đi qua, kể cả POC 1 org. Auth JWT đơn giản. Ingest hardening đầy đủ: MIME sniff, zip-bomb, size limit, quarantine, sandbox worker + timeout. Document state machine + tombstone + reconciliation. **Quota reserve→finalize/refund atomic trọn vẹn** (một cơ chế, không làm bản tạm) | Upload độc hại (extension giả, zip-bomb, quá size) bị chặn trước converter; kill worker giữa chừng → job resume đúng checkpoint; 2 request đồng thời không vượt quota | #5 consistency 3 hệ — vùng dễ bug nhất |
| **M2 — RAG** (map 1B gốc, phần retrieval) | Embedding queue vLLM (batch, backpressure, retry, index signature pin model+dim+version); Q&A hybrid: Qdrant top-k ∥ PG FTS → rerank → GLM SSE → citation re-check ACL + trạng thái doc; LLM lỗi → fallback trích đoạn | Đo recall trên golden-set đạt ngưỡng đã chốt ở Track A | #2 OCR/ingest cổ chai CPU, #4 GPU |
| **GATE POC** (= gate Phase 1B gốc, KHÔNG tách rời) | — | **Demo end-to-end single-org: ingest 1 tài liệu mỗi định dạng hỗ trợ → query ra citation đúng.** M1 và M2 không được báo "xong" riêng lẻ khi chưa qua gate chung này | — |
| **M3a — Web SPA MVP** (map Phase 2 gốc) | Login; thư viện (collection/upload/preview/xóa/reindex, trạng thái ingest realtime); chat Q&A citation SSE; admin tối thiểu (member/role/usage) | Demo end-to-end **qua UI** + Playwright E2E suite xanh (Track T) | — |
| **M3b — Intelligence port** (map Phase 3 gốc; song song hoặc sau M3a, KHÔNG chặn M4) | Tóm tắt, quality, PII/redaction, BA/PM handoff từ `intelligence.rs` (1.168 dòng) | Từng tính năng có test + demo riêng | — |
| **M4 — RBAC & multi-org** (map Phase 1C gốc) | Role/ACL mức 2 đầy đủ; rate limit 2 tầng (tower_governor + per-org quota dashboard); rollout multi-org; **denial test suite** phủ Qdrant search / PG FTS / citation fetch / preview / download | Denial test suite pass: user ngoài collection không nhận nội dung qua bất kỳ đường nào; vượt quota → 429 | — |
| **M5 — Hardening & rollout** (map Phase 4 gốc) | Audit review + pentest checklist; SSO/OIDC; trang hướng dẫn + onboarding; backup/recovery drill (PG backup, Qdrant snapshot, rebuild index từ PG) | Recovery drill thành công; pentest checklist đóng | #6 PhoWhisper license — kiểm tra trước khi bundle audio |

## Track T — Test automation (xuyên suốt, từ M1) [Đề xuất mới]

| Giai đoạn | Nội dung |
|---|---|
| Cùng B | Test khoá hành vi desktop trước khi tách crate (điều kiện gate B) |
| Cùng M1 | Integration test: state machine, reconciliation, quota atomic (concurrent), upload adversarial |
| Cùng M2 | Eval harness golden-set (recall/citation đúng) — chạy lại được mỗi lần đổi model/rerank |
| Cùng M3a | **Playwright E2E**: login → upload → theo dõi convert/index → Q&A → verify citation. CI dùng docker-compose + embedding hash-local fallback (không cần GPU) |
| Sau M3a | **Agent-driven browser test**: AI agent điều khiển browser (Playwright MCP) chạy exploratory smoke theo kịch bản ngôn ngữ tự nhiên trên staging — bắt lỗi UX/flow ngoài script cứng |
| Cùng M4 | Denial test suite (điều kiện gate M4) |

## Gợi ý phân nhóm giao task (bước kế tiếp: chẻ task chi tiết theo nhóm)

- **Backend-Infra**: A, C1a/C1b, M1 (adapter, state machine, reconciliation)
- **Backend-Rust-Core**: B, M2 (embedding queue, hybrid rerank)
- **Backend-API**: C2 (server), M1 (auth, tenant-scoped repo, quota), M4
- **Frontend**: C0, C2 (web), M3a
- **QA/Automation**: Track T (chủ trì), phối hợp gate từng milestone
- **DevOps/Security**: A (threat model), M1 (sandbox worker), M5

## Thay đổi so với draft đầu (theo review đối kháng nội bộ)

1. Tenant-scoped repository chuyển từ phase RBAC về **M1 Integration** — xây một lần, tránh rework mọi call-site.
2. Quota atomic reserve/finalize gộp thành **một cơ chế duy nhất ở M1** — không có bản "reserve tạm" check-then-act.
3. Thêm lại **cột Gate nhị phân** cho mọi track/milestone (kế thừa nguyên văn gate gốc).
4. A/B/C chạy **song song từ ngày 1**; PG schema không chờ Phase 0, chỉ Qdrant config chờ.
5. Core reuse (rủi ro cao nhất) đưa lên track song song sớm nhất thay vì xếp sau "nền tảng".
6. Đổi "Phase" tuần tự thành **Track (song song) + Milestone (hội tụ)**; thêm sơ đồ phụ thuộc.
7. M1+M2 chung **GATE POC** duy nhất — không báo xong riêng lẻ.
8. Web SPA MVP (M3a) tách khỏi Intelligence port (M3b) như bản gốc; M3b không chặn M4.
9. Styling + coding convention hạ xuống **C0 kickoff checklist**, không chiếm phase riêng.
10. Rủi ro gắn trực tiếp vào cột trong bảng thay vì mục rời cuối file.

## Câu hỏi mở (cần user chốt trước/trong khi chia task)

1. Quy ước code server+web: phụ lục trong `docs/code-standards.md` hay file `docs/web-code-standards.md` riêng?
2. Danh sách định dạng upload cho phép ở POC: đủ bộ core hỗ trợ hay giới hạn pdf/docx/xlsx trước?
3. Nhân sự thực tế cho các nhóm ở mục "Gợi ý phân nhóm" (số người, ai biết Rust) — quyết định độ mịn khi chẻ task.
4. Hạ tầng: GPU server (VRAM), provisioning PG/Qdrant/MinIO, GLM endpoint — điều kiện bắt đầu Track A.
