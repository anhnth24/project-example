# Phase 3 — Document Intelligence trên web

## Outcome

Port các chức năng intelligence đã có ở desktop sang service/artifact model của
web: BRD/PRD handoff, quality, PII/redaction, schema/tables, versions và export.

## P3.1 — Extract service boundary

Tái dùng deterministic logic trong `crates/core/src/intelligence.rs`; không copy
thuật toán sang server. Tách orchestration đang nằm ở
`app/src-tauri/src/intelligence.rs` thành service nhận:

- `OrgContext`;
- immutable document versions;
- artifact store;
- optional LLM policy/config;
- job/audit handles.

Desktop giữ thin filesystem adapter. Web adapter dùng PostgreSQL/MinIO.

## P3.2 — Derived artifact model

Mỗi output là artifact versioned:

- type, status và schema version;
- source document-version IDs;
- source ACL snapshot/policy;
- content hash/index signature;
- creator/job/model/prompt template version;
- citations;
- audit metadata.

Artifact kế thừa intersection ACL của nguồn theo policy đã chốt. Mỗi lần
read/download/export phải tính hoặc validate **current** intersection ACL và trạng
thái của mọi source; invalidation job chỉ cleanup cache, không phải security
boundary. Không dùng public MinIO URL.

## P3.3 — BRD/PRD handoff studio

Server jobs:

- generate deterministic pack;
- optional LLM enhancement chỉ trên corpus đã authorize;
- validate citation/ID/traceability;
- save/edit artifact version;
- export ZIP.

Pack:

- README;
- BRD;
- PRD;
- user stories;
- acceptance criteria;
- glossary;
- test cases;
- traceability;
- assumptions/open questions;
- manifest/validation.

Invariant:

- không có API key vẫn sinh được;
- factual requirement phải có citation;
- thiếu dữ liệu → assumption/open question, không bịa.

## P3.4 — Quality và hard reprocess

- Quality report per document/block.
- Recommendation cho OCR/reconvert.
- Reprocess tạo version/job mới, không ghi đè canonical.
- Optional hard OCR/VLM qua worker policy, quota và egress control.
- So sánh trước/sau bằng quality metrics và lưu provenance.

## P3.5 — Summarization

- Tóm tắt theo document, collection hoặc selected corpus.
- Deterministic extractive fallback không cần LLM.
- LLM summary chỉ dùng retrieved/authorized passages và giữ citation.
- Length/audience/language là explicit options; không thêm fact không có nguồn.
- Golden test phủ factuality, citation coverage và câu hỏi không đủ dữ liệu.

## P3.6 — PII và redaction

- Scan email, phone, CCCD/CMND, bank-like values và rule mở rộng.
- Permission riêng `pii.manage`.
- Report và export được audit.
- Redaction tạo derived document/version mới.
- Original immutable.
- UI buộc review trước publish/export.

Không gửi tài liệu có classification cấm ra GLM cloud.

## P3.7 — Schema, tables và versions

- Extract document schema.
- List/edit Markdown table bằng stable table/cell IDs.
- Export CSV có phòng formula injection.
- Snapshot/version list.
- Diff và three-way merge; conflict rõ ràng.
- Optimistic concurrency khi save artifact/table.
- Version timeline hiển thị effective/current state, deterministic field/fact changes
  và citation deep-link cho từng immutable version.
- Intelligence artifact ghi exact source version IDs; summary/handoff compare nhiều
  version phải cite old+new, không silently refresh sang current.

Các tính năng schema/table/version/export là scope desktop intelligence đã có trong
`plans/260710-document-intelligence-suite.md`; Phase 3 port tương thích, không mở
thêm workflow mới.

## P3.8 — Intelligence UI decomposition

Tách `IntelligenceView.tsx` thành:

- `CorpusPanel`;
- `HandoffStudio`;
- `QualityPanel`;
- `VersionsPanel`;
- `TablesPanel`;
- `PrivacyPanel`;
- `ExportPanel`.

Q&A panel đã thuộc Phase 2. Không port watch-folder desktop. Browser download thay
native save dialog; citation deep-link qua document route.

## P3.9 — Prompt/LLM security

- System instruction và retrieved text phân ranh rõ.
- Document text không được gọi tool, đổi ACL hoặc scope.
- Per-org cloud egress policy và token cap.
- Audit provider/model/classification/usage, không log prompt body.
- Timeout/cancel/fallback deterministic.
- Model/prompt rollout dùng versioned template và golden eval.

## P3.10 — Quality evaluation

Golden sets riêng:

- citation coverage và validity;
- requirement extraction/traceability;
- PII precision/recall;
- redaction completeness;
- table round-trip;
- merge conflict correctness.

Retrieval metric không thay thế task-specific intelligence metric.

## Tests

- Handoff offline, LLM fallback và citation invariant.
- Artifact ACL/revoke/cross-org denial.
- PII original không đổi.
- Table edit không phá surrounding text.
- Version diff/merge và optimistic conflict.
- Export manifest/ACL/formula safety.
- Prompt injection và cloud-policy deny.
- Component/E2E cho từng panel.

## Gate

- BRD/PRD pack hoàn chỉnh từ corpus thực, mọi factual requirement có citation.
- Zero cross-scope derived artifacts.
- PII/redaction đạt threshold đã duyệt; canonical không bị mutate.
- Table/version/export round-trip đúng.
- Deterministic fallback hoạt động khi provider lỗi.
- Audit coverage đầy đủ cho intelligence/PII/export/cloud egress.

## Không thuộc phase

- Server-side watch folder.
- Custom agent/tool execution từ nội dung tài liệu.
- OIDC/group synchronization.
