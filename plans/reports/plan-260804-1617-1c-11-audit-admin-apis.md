<!-- generated-done-issue-plan: 1C-11 -->
# 1C-11 — Audit/admin APIs

Date: 2026-08-04
Source issue: [#113](https://github.com/anhnth24/project-example/issues/113)
Catalog: [`backlog/phase-1c/issues/README.md`](../markhand-web/backlog/phase-1c/issues/README.md)
Phase plan: [`phase-1c-multi-org-security.md`](../markhand-web/phase-1c-multi-org-security.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Audit/admin APIs**.

## Context

- Phase: `1C`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> CI exact-SHA evidence on `6833f57d94949c75ea36609e1055a1139e097c8a`
> (run [30678318560](https://github.com/anhnth24/project-example/actions/runs/30678318560)):
> `rust-integration`
> [91310110925](https://github.com/anhnth24/project-example/actions/runs/30678318560/job/91310110925)
> (not path-filter skip / soft pass) for audit coverage, **plus** owner-approved
> accepted risk `AR-1C-AUDIT-RETENTION` recorded in PR 1 / P1C.6 (POC/
> non-production only; Phase 4 owns retention/TTL/tamper/export — **retention is
> not implemented here**). Integration log executed `tests/audit_read.rs`
> (**9 passed; 0 failed**) including pagination, action/actor/time filters,
> `audit.view` 403 + deny-audit row, cross-org isolation, and metadata allowlist
> redaction. Direct-service `audit_view_permission_required_at_direct_list_page`
> green in `tests/direct_service_authz.rs`.

## Implementation plan

Member/role/ACL/config/quota/data/cloud events (**out of scope của
đợt này — chỉ audit READ**); read-only pagination/filter (**done**: `routes/audit.rs`,
`db/audit.rs::list_page`)/retention (**out**); owner-only controls (**out — không
phải đọc log**).

## Files/modules

The source catalog records implementation and file scope together; see **Implementation plan** above.

## Dependencies / blocks

1C-02…10.

## Acceptance criteria

Mọi mutation có actor/org/action/target/
result/request ID (pre-existing, không đổi ở đây); coverage (pre-existing, không
đổi)/access (**done**: 403 + deny-audit test)/pagination (**done**: cursor stable
test)/redaction (**done**: no-leak-beyond-allowlist test)/retention (**out of
scope**, chưa làm).

## Required tests / evidence

Mọi mutation có actor/org/action/target/
result/request ID (pre-existing, không đổi ở đây); coverage (pre-existing, không
đổi)/access (**done**: 403 + deny-audit test)/pagination (**done**: cursor stable
test)/redaction (**done**: no-leak-beyond-allowlist test)/retention (**out of
scope**, chưa làm).

## Security and migration notes

No document/prompt/token/PII/URL (allowlist per-action giữ
nguyên, `audit.read` chỉ thêm `result_count`). Không migration mới — RLS/seed

## Out of scope

SIEM archive, retention/TTL, audit coverage mở
rộng sang action mới cho ACL/config/quota/cloud mutation, owner-only admin
controls khác ngoài đọc log.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- `6833f57d94949c75ea36609e1055a1139e097c8a`

- GitHub sync-closed timestamp: `2026-08-01T04:06:40Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
