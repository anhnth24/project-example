# Architecture Decision Records

ADR ghi quyết định khó đảo ngược hoặc ảnh hưởng nhiều boundary: dependency direction,
tenant/RLS, storage/vector topology, auth/session, public API compatibility, runtime
native, security/egress và migration strategy.

## Quy trình

1. Tạo `NNNN-slug.md` từ [`TEMPLATE.md`](TEMPLATE.md): Context, Decision,
   Consequences, Alternatives, Verification, Owners/Approver.
2. Giữ trạng thái `Proposed` cho tới khi owner/approver review evidence.
3. Đổi sang `Accepted`, `Rejected`, hoặc `Superseded by ADR NNNN`; không sửa lại
   history để làm mất lý do cũ.
4. PR thay đổi public contract hoặc security boundary phải link ADR; exception cần
   expiry và regression test.

## Index

| ADR | Status | Decision |
|---|---|---|
| [0001](0001-web-boundaries.md) | Accepted | Dependency boundaries của Markhand Web |
| [0002](0002-version-aware-citations.md) | Accepted | Version-aware retrieval and citations |
| [0003](0003-cross-document-conflict-lifecycle.md) | Proposed | Cross-document claim/conflict lifecycle |
| [0004](0004-interim-glm-cloud-embedding.md) | Superseded by 0005 | GLM cloud embedding interim (historical; superseded 2026-07-20) |
| [0005](0005-vietnamese-embedding-model-quality.md) | Accepted | AITeamVN local embedding for POC/1B; GLM chat-only |
| [0006](0006-index-signature.md) | Accepted | Canonical index signature and chunk identity (P0-06) |
| [0007](0007-tenant-isolation-rls.md) | Accepted | Tenant isolation and RLS boundary |
| [0008](0008-pg-partition-strategy.md) | Accepted | PG partition strategy for Phase 1B POC and Profile B revalidation |
| [0009](0009-qdrant-topology.md) | Accepted | Qdrant topology for Phase 1B POC and Profile B revalidation |
| [0010](0010-auth-session-lifecycle.md) | Accepted | Auth session lifecycle for Phase 1B |
| [0011](0011-model-index-migration.md) | Accepted | Model and index migration lifecycle |
| [0012](0012-backup-recovery-order.md) | Accepted | Backup and recovery authority/order |
| [0013](0013-intelligence-durable-ids.md) | Accepted | Durable intelligence IDs (`sha256-v1`) and desktop rebuild |
| [0014](0014-vietnamese-word-segmentation-fts.md) | Proposed | Vietnamese word segmentation for FTS lexical retrieval (P1B-R01) |
| [0015](0015-purge-content-retention-semantics.md) | Proposed | Immutable-content retention semantics for document purge (P1B-I07) |

Phase 0 numeric/benchmark decisions use the machine-readable
[`bench/markhand_web/gates.yaml`](../../bench/markhand_web/gates.yaml) registry.
Approved gate evidence may result in an ADR, but threshold values are not duplicated
inside this index.
