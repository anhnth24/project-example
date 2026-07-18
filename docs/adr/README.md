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
| [0002](0002-version-aware-citations.md) | Proposed | Version-aware retrieval and citations |
| [0003](0003-cross-document-conflict-lifecycle.md) | Proposed | Cross-document claim/conflict lifecycle |
| [0004](0004-interim-glm-cloud-embedding.md) | Accepted | Interim GLM cloud embedding; target on-prem vLLM |
| [0005](0005-vietnamese-embedding-model-quality.md) | Proposed | Local dense quality candidates for P0-05 |
| [0006](0006-index-signature.md) | Accepted | Canonical index signature and chunk identity (P0-06) |
| [0008](0008-pg-partition-strategy.md) | Accepted | PG partition strategy for Phase 1B POC and Profile B revalidation |
| [0009](0009-qdrant-topology.md) | Accepted | Qdrant topology for Phase 1B POC and Profile B revalidation |

Phase 0 numeric/benchmark decisions use the machine-readable
[`bench/markhand_web/gates.yaml`](../../bench/markhand_web/gates.yaml) registry.
Approved gate evidence may result in an ADR, but threshold values are not duplicated
inside this index.
