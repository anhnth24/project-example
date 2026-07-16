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

## Phạm vi các phase

| Phase | Kết quả chính | Tài liệu |
|---|---|---|
| F | Engineering rules, skeleton, local dev environment và CI foundation | [`phase-f-engineering-foundation.md`](phase-f-engineering-foundation.md) |
| 0 | Chốt bằng số liệu: scale, retrieval, bảo mật upload, SLA/RPO/RTO | [`phase-0-discovery-and-gates.md`](phase-0-discovery-and-gates.md) |
| 1A | Tách logic RAG dùng chung thành `crates/knowledge`, desktop không đổi hành vi | [`phase-1a-knowledge-extraction.md`](phase-1a-knowledge-extraction.md) |
| 1B | POC single-org hoàn chỉnh: upload → convert → index → Q&A citation | [`phase-1b-single-org-poc.md`](phase-1b-single-org-poc.md) |
| 1C | Multi-org, RBAC/ACL, quota atomic và denial test | [`phase-1c-multi-org-security.md`](phase-1c-multi-org-security.md) |
| 2 | Web SPA MVP: login, library, Q&A, admin tối thiểu | [`phase-2-web-spa.md`](phase-2-web-spa.md) |
| 3 | Port intelligence: BRD/PRD, quality, PII, bảng, version, export | [`phase-3-intelligence.md`](phase-3-intelligence.md) |
| 4 | OIDC/SSO, hardening production, DR và onboarding/help | [`phase-4-production-hardening.md`](phase-4-production-hardening.md) |

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

- GPU/VRAM, model embedding và throughput thực tế.
- SLA/SLO, RPO/RTO và retention backup.
- Format/giới hạn upload của POC.
- Qdrant shared collection hay phân cohort.
- PostgreSQL partition strategy và việc bắt buộc RLS.
- Canonical storage của Markdown/derived artifacts.
- Chính sách GLM cloud theo phân loại dữ liệu.
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
