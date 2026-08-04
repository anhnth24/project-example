<!-- generated-done-issue-plan: 1C-09 -->
# 1C-09 — Atomic quota lifecycle

Date: 2026-08-04
Base commit: UNKNOWN — not recorded in the source catalog
Source issue: [#111](https://github.com/anhnth24/project-example/issues/111)
Catalog: [`backlog/phase-1c/issues/README.md`](../markhand-web/backlog/phase-1c/issues/README.md)
Phase plan: [`phase-1c-multi-org-security.md`](../markhand-web/phase-1c-multi-org-security.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Atomic quota lifecycle**.

## Context

- Phase: `1C`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> (run [30678318560](https://github.com/anhnth24/project-example/actions/runs/30678318560)):
> `rust-integration`
> [91310110925](https://github.com/anhnth24/project-example/actions/runs/30678318560/job/91310110925)
> (not path-filter skip / soft pass). Integration log executed
> `tests/quota.rs` (**16 passed; 0 failed**) including
> `concurrent_reserve_does_not_over_reserve`,
> `job_claim_enforces_and_releases_concurrent_slots`,
> `finalize_actual_commits_measured_token_usage`,
> `reconcile_repairs_counter_drift_and_orphaned_job_slots`,
> `upload_two_resource_settlement_is_atomic`, and idempotency/expiry/overflow
> paths. Embedding-provider token metering remains backlog (out of issue scope).

## Implementation plan

Reserve/finalize/refund, idempotency/expiry/sweeper/reconcile cho
storage/token/jobs.

## Files/modules

The source catalog records implementation and file scope together; see **Implementation plan** above.

## Dependencies / blocks

Phase 1B jobs + 1C-01.

## Acceptance criteria

100 concurrent reservations
không over-limit; crash/retry/cancel/timeout/actual-usage tests.

## Required tests / evidence

100 concurrent reservations
không over-limit; crash/retry/cancel/timeout/actual-usage tests.

## Security and migration notes

Checked arithmetic, org/resource unique key.

## Out of scope

billing.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Completion/evidence commits

- `30678318560`
- `6833f57d94949c75ea36609e1055a1139e097c8a`
- `91310110925`

- GitHub sync-closed timestamp: `2026-08-01T04:06:35Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
