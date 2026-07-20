# Markhand Web — kế hoạch triển khai theo phase

Ngày lập: 2026-07-16
Nguồn thiết kế đã duyệt:
[`../reports/brainstorm-260713-1656-markhand-web-rag-multi-org-report.md`](../reports/brainstorm-260713-1656-markhand-web-rag-multi-org-report.md)

## Mục tiêu

Xây Markhand Web on-prem trên nền `fileconv-core`: quản lý tài liệu, chuyển đổi,
index, tìm kiếm hybrid, hỏi đáp có citation và bộ công cụ intelligence; hỗ trợ
multi-org, multi-user, RBAC và quota.

Kế hoạch này biến report kiến trúc thành các gói việc có dependency, deliverable,
test và gate đo được. Không dùng thời gian lịch làm tiêu chí; chỉ chuyển phase khi
gate kỹ thuật của phase trước đã đạt.

Issue-level backlog (113 issues):
[`backlog/README.md`](backlog/README.md).

Roadmap dashboard tương tác:
[`roadmap.html`](roadmap.html) — lọc/tìm kiếm, cập nhật trạng thái, lưu local và
export/import JSON.

File HTML nằm ngay trong repo. Generator bắt đầu từ phase registry ở bảng trên,
theo link tới từng phase plan và issue catalog, rồi đọc tiêu đề/`**Status:**` của
từng issue. Generator cũng đọc bảng **Technology stack** để dựng tab cùng tên.
Chạy các lệnh sau từ repository root sau khi đổi registry, stack, tiêu đề hoặc trạng
thái issue:

```bash
python3 scripts/build-roadmap.py
python3 scripts/build-roadmap.py --check
```

Mỗi catalog có `roadmap-default-status`; `**Status:**` trong issue sẽ override giá
trị mặc định. Source hash mới tạo namespace local riêng nên trạng thái Markdown mới
không bị local override cũ che mất.

## Phạm vi các phase

| Phase | Kết quả chính | Phase plan | Issue catalog |
|---|---|---|---|
| F | Engineering rules, skeleton, local dev environment và CI foundation | [Phase plan](phase-f-engineering-foundation.md) | [12 issues](backlog/phase-f/issues/README.md) |
| 0 | Chốt bằng số liệu: scale, retrieval, bảo mật upload, SLA/RPO/RTO | [Phase plan](phase-0-discovery-and-gates.md) | [10 issues](backlog/phase-0/issues/README.md) |
| 1A | Tách logic RAG dùng chung thành `crates/knowledge`, desktop không đổi hành vi | [Phase plan](phase-1a-knowledge-extraction.md) | [10 issues](backlog/phase-1a/issues/README.md) |
| 1B | POC single-org hoàn chỉnh: upload → convert → index → Q&A citation | [Phase plan](phase-1b-single-org-poc.md) | [24 issues](backlog/phase-1b/issues/README.md) |
| 1C | Multi-org, RBAC/ACL, quota atomic và denial test | [Phase plan](phase-1c-multi-org-security.md) | [13 issues](backlog/phase-1c/issues/README.md) |
| 2 | Web SPA MVP: login, library, Q&A, admin tối thiểu | [Phase plan](phase-2-web-spa.md) | [16 issues](backlog/phase-2/issues/README.md) |
| 3 | Port intelligence: BRD/PRD, quality, PII, bảng, version, export | [Phase plan](phase-3-intelligence.md) | [14 issues](backlog/phase-3/issues/README.md) |
| 4 | OIDC/SSO, hardening production, DR và onboarding/help | [Phase plan](phase-4-production-hardening.md) | [14 issues](backlog/phase-4/issues/README.md) |

## Dependency và đường găng

```text
Phase F ─┬─> Phase 0 ─┐
         └─> Phase 1A ┴─> Phase 1B ─> Phase 1C ────────> Phase 2 complete
                                  └─> stable OpenAPI ─> Phase 2 UI/mock
                                                 Phase 2 complete ─> Phase 3 ─> Phase 4
```

- Phase F phải pass trước khi activate Phase 0/1A.
- Sau Phase F, Phase 0 và 1A có thể làm song song.
- 1B không được fork/copy logic RAG desktop; phải dùng kết quả 1A.
- 1B dùng một org nhưng mọi repository, object key, job, vector và event bắt buộc
  mang `OrgContext`; 1C mở nhiều org và hoàn thiện chính sách, không retrofit tenancy.
- Phase 2 có thể phát triển UI với OpenAPI mock khi API 1B ổn định, nhưng chỉ đạt
  gate sau khi backend 1C qua denial suite.
- Phase 3 chỉ port intelligence sau khi auth, ACL, artifact model và audit đã ổn định.

## Kiến trúc đích

```text
web/ (React + Vite)
        │ REST + SSE
crates/server (axum)
        ├── PostgreSQL: system of record, FTS, auth, jobs, quota, audit
        ├── Qdrant: vector candidates
        ├── MinIO: file gốc, Markdown và artifact
        └── workers: convert / embed / delete / reconcile
              │
crates/knowledge: rank, merge, citation, grounding, index signature
              │
crates/core: convert, chunk, deterministic intelligence, LLM/embedding clients
```

PostgreSQL luôn là nguồn sự thật. Qdrant có thể rebuild từ chunk trong PostgreSQL;
MinIO cần backup riêng vì file gốc không thể tái tạo từ index.

## Technology stack

Bảng này là nguồn dữ liệu cho tab **Tech stack** trong
[`roadmap.html`](roadmap.html).

<!-- roadmap-tech-stack-start -->
| Lớp | Công nghệ | Trách nhiệm | Delivery |
|---|---|---|---|
| Web client | React + Vite + TypeScript | SPA cho library, upload, search, Q&A và admin | Phase 2 |
| API | Rust + axum + OpenAPI | REST API, SSE progress, auth middleware và OrgContext | Phase 1B |
| Shared knowledge | Rust crate knowledge | Hybrid rank/merge, grounding, citation và index signature | Phase 1A |
| Document engine | fileconv-core | Convert, OCR, chunk và deterministic intelligence | Existing core |
| System of record | PostgreSQL + FTS | Metadata, ACL, auth, jobs, quota, audit và lexical search | Phase 1B |
| Vector retrieval | Qdrant | Vector candidates; kết quả luôn được hydrate và kiểm ACL lại | Phase 1B |
| Object storage | MinIO | File gốc, quarantine, Markdown và derived artifacts | Phase 1B |
| Embeddings | AITeamVN local → on-prem vLLM | POC/1B: `AITeamVN/Vietnamese_Embedding` on-prem CPU (`local-neural`, Compose `:8088`); target: vLLM GPU self-host; cắt sang vLLM = rebuild index | Phase 0 → 1B (local); cutover trước production |
| Chat and extraction | GLM via LLM client | Grounded Q&A, summarize và structured extraction theo policy (**không** dùng cho embedding/index) | Phase 1B → 3 |
| Identity | JWT + rotating refresh + OIDC | Session cho POC; SSO/OIDC và key rotation cho production | Phase 1B → 4 |
| Observability | OpenTelemetry + structured logs | Trace, metrics, audit correlation và redacted diagnostics | Phase F → 4 |
| Runtime | Docker Compose → production orchestrator | Local/POC reproducible; Kubernetes hoặc nền tảng on-prem tương đương được chốt ở Phase 4 | Phase F → 4 |
<!-- roadmap-tech-stack-end -->

## Invariant xuyên suốt

1. Không endpoint, repository hay adapter nào chạy khi thiếu org context.
2. Candidate retrieval luôn được hydrate và kiểm quyền lại từ PostgreSQL trước khi
   trả text/citation.
3. Xóa/revoke có hiệu lực tức thời ở read path; dọn Qdrant/MinIO chạy idempotent sau.
4. File upload đi qua quarantine và converter cô lập trước khi trở thành tài liệu tin cậy.
5. Job dùng lease, checkpoint, idempotency key; retry không tạo chunk/artifact trùng.
6. Không log nội dung tài liệu, prompt, token, API key, signed URL hay PII.
7. Model, dimension, normalize, chunking version và index signature được pin.
8. Derived artifacts kế thừa ACL nguồn; redaction không ghi đè bản gốc.
9. Migration dùng expand/cutover/contract; rollback ứng dụng không yêu cầu rollback DB.
10. Desktop tiếp tục build/test trong mọi phase.

## Definition of done chung

- Unit, integration, contract, E2E và denial tests tương ứng đều xanh.
- Migration chạy được từ DB rỗng và từ release được hỗ trợ.
- Có metrics/traces/audit phù hợp, không chứa dữ liệu nhạy cảm.
- Tài liệu vận hành và rollback được cập nhật cùng thay đổi.
- Zero unresolved high/critical findings; accepted risk phải có approver,
  compensating controls, expiry và retest date.

## Quyết định còn mở — Phase 0 phải chốt

- On-prem GPU/VRAM và throughput vLLM khi cutover (embedding POC/1B dùng AITeamVN local — ADR 0005).
- SLA/SLO, RPO/RTO và retention backup.
- Format/giới hạn upload của POC.
- Qdrant shared collection hay phân cohort.
- PostgreSQL partition strategy và việc bắt buộc RLS.
- Canonical storage của Markdown/derived artifacts.
- Chính sách GLM cloud theo phân loại dữ liệu (Q&A/summarize; **không** embedding server;
  customer data không ra cloud cho index build).
- JWT signing/key rotation/session/MFA.
- ACL chi tiết cho private/org/groups.
- License PhoWhisper khi deploy server.

## Phạm vi POC

POC được coi là hoàn thành sau **Phase 1B**, không phải chỉ dựng API:

- Một org và vài account test.
- Mỗi format đã allowlist có ít nhất một file chạy end-to-end.
- Search/Q&A trả citation đã kiểm quyền.
- Worker bị kill có thể resume từ checkpoint.
- Corpus upload độc hại bị chặn hoặc chứa trong sandbox.
- Backup/restore trên môi trường sạch đã chạy thành công.

Phase 1C là gate bắt buộc trước khi mở hệ thống cho nhiều org hoặc người dùng không
thuộc cùng một nhóm tin cậy.
