# Markhand Web — kế hoạch triển khai theo phase

Ngày lập: 2026-07-16 · Cập nhật milestone/issue: 2026-08-01
Nguồn thiết kế đã duyệt:
[`../reports/brainstorm-260713-1656-markhand-web-rag-multi-org-report.md`](../reports/brainstorm-260713-1656-markhand-web-rag-multi-org-report.md)

## Mục tiêu

Xây Markhand Web on-prem trên nền `fileconv-core`: quản lý tài liệu, chuyển đổi,
index, tìm kiếm hybrid, hỏi đáp có citation và bộ công cụ intelligence; hỗ trợ
multi-org, multi-user, RBAC và quota.

Kế hoạch này biến report kiến trúc thành các gói việc có dependency, deliverable,
test và gate đo được. Không dùng thời gian lịch làm tiêu chí; chỉ chuyển phase khi
gate kỹ thuật của phase trước đã đạt.

Issue-level backlog (120 issues):
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
| 2 | Web SPA MVP: login, library, Q&A, admin tối thiểu | [Phase plan](phase-2-web-spa.md) | [23 issues](backlog/phase-2/issues/README.md) |
| 3 | Port intelligence: BRD/PRD, quality, PII, bảng, version, export | [Phase plan](phase-3-intelligence.md) | [14 issues](backlog/phase-3/issues/README.md) |
| 4 | OIDC/SSO, hardening production, DR và onboarding/help | [Phase plan](phase-4-production-hardening.md) | [14 issues](backlog/phase-4/issues/README.md) |

## Tiến độ milestone (2026-08-06)

Tổng **120 issue** trong catalog: **81 Done**, **5 In progress**, **1 Review**,
**0 Ready**, **4 Blocked**, **29 Backlog**.
Nguồn sự thật là `**Status:**` trong từng issue catalog; bảng dưới tóm tắt theo phase.
GitHub milestone progress được đồng bộ bởi workflow
[`Sync Markhand Web issues`](../../.github/workflows/sync-markhand-issues.yml) khi
[`github-issues.json`](backlog/github-issues.json) thay đổi trên `master`.

| Phase | Done | Active (In progress + Review) | Ready | Blocked | Backlog | Gate / ghi chú |
|---|---:|---:|---:|---:|---:|---|
| F | 12/12 | 0 | 0 | 0 | 0 | Foundation gate đạt |
| 0 | 10/10 | 0 | 0 | 0 | 0 | Discovery gates đạt |
| 1A | 10/10 | 0 | 0 | 0 | 0 | Extraction gate đạt |
| 1B | 24/24 | 0 | 0 | 0 | 0 | **Gate đạt** — R06 hanging soak pass 2026-07-31 |
| 1C | 10/13 | 2 | 0 | 0 | 1 | 1C-04/07/09/10/11 Done (CI `6833f57`, run 30678318560); 1C-08 CI half / deployed → PR 5; exit gate 1C-12/1C-13 còn mở |
| 2 | 15/23 | 4 | 0 | 4 | 0 | P2-15 umbrella blocked; P2-20 `Review` (PR #395); exit gate chờ full-stack evidence + 1C + blocking real/DAST matrix |
| 3 | 0/14 | 0 | 0 | 0 | 14 | Chưa activate — chờ Phase 2 complete |
| 4 | 0/14 | 0 | 0 | 0 | 14 | Chưa activate — chờ Phase 3 |

**Critical path hiện tại:** P2-20 real fixture/foundation ở `Review` (chờ
`DEV_STACK_MODE=full` + `make check-web`/`check-desktop` trước `Done`); song song hoàn
thiện P2-18/P2-19 và 1C denial suite (1C-12/1C-13) để mở P2-21/P2-22/P2-23 →
P2-15 umbrella → P2-16 final gate.

Issue Phase 2 gần đây: **P2-10/P2-17 → Done** sau independent review; P2-20 Tasks 1–8
landed trên [#395](https://github.com/anhnth24/project-example/pull/395) với whole-branch
review APPROVE; P2-18 Project grouping và P2-19 Chat history vẫn In progress. P2-21…23
blocked theo dependency/security gate.

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
| Document engine | fileconv-core | Convert, chunk và deterministic intelligence; OCR qua vision-LLM (deferred trong sandbox) | Existing core |
| Vision OCR | OpenRouter Qwen vision (`MARKHAND_OCR_*`) | OCR ảnh/trang scan ở worker stage ngoài sandbox; Tesseract đã loại bỏ (ADR 0016) | Delivered 2026-08-10 |
| System of record | PostgreSQL + FTS | Metadata, ACL, auth, jobs, quota, audit và lexical search | Phase 1B |
| Vector retrieval | Qdrant | Vector candidates; kết quả luôn được hydrate và kiểm ACL lại | Phase 1B |
| Object storage | MinIO | File gốc, quarantine, Markdown và derived artifacts | Phase 1B |
| Embeddings | AITeamVN local / OpenRouter `qwen3-embedding-8b` | POC/1B baseline: AITeamVN on-prem CPU (`local-neural`, Compose `:8088`, Recall@5 0.9261). ADR 0016 thay target vLLM GPU bằng OpenRouter `provider-cloud` (egress opt-in `MARKHAND_ALLOW_CLOUD_EMBEDDINGS`); thành pin mặc định sau benchmark golden corpus (DKP-02); đổi runtime = rebuild index generation | Phase 0 → 1B (local); OpenRouter option delivered 2026-08-10 |
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

## Quyết định còn mở

Đã chốt 2026-08-10 (ADR 0016) và loại khỏi danh sách: GPU/VRAM/throughput vLLM
cutover (bỏ — thay bằng OpenRouter, gate `G0-RET-VLLM-CUTOVER` retired); chính
sách cloud embedding (được phép sau egress opt-in tường minh
`MARKHAND_ALLOW_CLOUD_EMBEDDINGS=true`; GLM vẫn chỉ Q&A).

Còn mở:

- Benchmark `qwen/qwen3-embedding-8b` trên golden corpus trước khi thành pin
  mặc định (DKP-02); quyết định dimension 4096 vs 1024 MRL.
- SLA/SLO, RPO/RTO và retention backup.
- Format/giới hạn upload của POC.
- Qdrant shared collection hay phân cohort.
- PostgreSQL partition strategy và việc bắt buộc RLS.
- Canonical storage của Markdown/derived artifacts.
- JWT signing/key rotation/session/MFA.
- ACL chi tiết cho private/org/groups.
- License PhoWhisper khi deploy server (audio).
- Data classification nào được phép qua OpenRouter per-org (OCR gửi ảnh trang;
  embedding gửi chunk text) — thiết kế opt-out per-org.

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
