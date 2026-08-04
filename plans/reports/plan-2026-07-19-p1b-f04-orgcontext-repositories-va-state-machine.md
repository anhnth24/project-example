<!-- generated-done-issue-plan: P1B-F04 -->
# P1B-F04 — OrgContext, repositories và state machine

Issue closed: 2026-07-19
Source issue: [#81](https://github.com/anhnth24/project-example/issues/81)
Catalog: [`backlog/phase-1b/issues/README.md`](../markhand-web/backlog/phase-1b/issues/README.md)
Phase plan: [`phase-1b-single-org-poc.md`](../markhand-web/phase-1b-single-org-poc.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **OrgContext, repositories và state machine**.

## Context

- Phase: `1B`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> Catalog records status as Done.

## Implementation plan

Tenant-scoped repos, transaction helpers, legal document transitions;
transaction-local RLS context nếu chọn.

## Files/modules

`src/auth/context.rs`, `src/db/{orgs,collections,documents,chunks}.rs`,
`src/services/document_state.rs`.

## Dependencies / blocks

F03 + G0-ARCH.

## Acceptance criteria

Không public business method thiếu context; cross-org deny;
invalid/concurrent transition atomic; pool leakage test.

## Required tests / evidence

Không public business method thiếu context; cross-org deny;
invalid/concurrent transition atomic; pool leakage test.

## Security and migration notes

Empty scope fail closed.

## Out of scope

Full ACL semantics 1C.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- UNKNOWN — no completion/evidence commit is cited in the catalog status.

- GitHub sync-closed timestamp: `2026-07-19T15:17:15Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
