<!-- generated-done-issue-plan: P1B-I02 -->
# P1B-I02 — Atomic quota admission

Issue closed: 2026-07-19
Source issue: [#85](https://github.com/anhnth24/project-example/issues/85)
Catalog: [`backlog/phase-1b/issues/README.md`](../markhand-web/backlog/phase-1b/issues/README.md)
Phase plan: [`phase-1b-single-org-poc.md`](../markhand-web/phase-1b-single-org-poc.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Atomic quota admission**.

## Context

- Phase: `1B`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> Catalog records status as Done.

## Implementation plan

Transactional reserve/finalize/refund, expiry, concurrent-job admission,
quota headers/errors.

## Files/modules

`src/db/quota.rs`, `services/quota.rs`, quota middleware.

## Dependencies / blocks

F03/F04/I01 + G0-CAP.

## Acceptance criteria

Concurrent requests không over-reserve; every terminal path
settles; expiry/retry/crash/overflow tests.

## Required tests / evidence

Concurrent requests không over-reserve; every terminal path
settles; expiry/retry/crash/overflow tests.

## Security and migration notes

Checked arithmetic, client không sửa counter.

## Out of scope

billing.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- UNKNOWN — no completion/evidence commit is cited in the catalog status.

- GitHub sync-closed timestamp: `2026-07-19T15:17:27Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
