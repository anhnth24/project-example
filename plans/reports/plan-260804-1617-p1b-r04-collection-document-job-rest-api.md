<!-- generated-done-issue-plan: P1B-R04 -->
# P1B-R04 — Collection/document/job REST API

Date: 2026-08-04
Base commit: UNKNOWN — not recorded in the source catalog
Source issue: [#94](https://github.com/anhnth24/project-example/issues/94)
Catalog: [`backlog/phase-1b/issues/README.md`](../markhand-web/backlog/phase-1b/issues/README.md)
Phase plan: [`phase-1b-single-org-poc.md`](../markhand-web/phase-1b-single-org-poc.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Collection/document/job REST API**.

## Context

- Phase: `1B`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> rust-integration (`b5cc92c`, run
> [30603158015](https://github.com/anhnth24/project-example/actions/runs/30603158015)/job
> [91070008980](https://github.com/anhnth24/project-example/actions/runs/30603158015/job/91070008980)),
> including cross-tenant IDOR/403 fixture corrections. Note:
> `test-hooks`-only audit rollback tests
> (`live_patch_collection_audit_correlation_and_rollback`,
> `live_reindex_audit_failure_rolls_back_enqueue`) are excluded from the normal
> rust-integration build (feature not enabled in CI), so evidence does not
> cover that gated subset. Sol R3 upload saga retained;
> `live_http_collection_document_job_contract_matrix` asserts reindex same
> `jobId` with `created=false` on idempotent replay. Business API mutations
> gated by central `mutation_write_gate` middleware (see O03).

## Implementation plan

`/api/v1` collection POC; upload/list/get/preview/delete/reindex; immutable
version list/get/diff/current publish; conflict list/detail/triage + evidence routes;
job status; pagination/idempotency/error schema.

## Files/modules

`routes/{collections,documents,jobs}.rs`, `api/{types,error,pagination}.rs`,
`tests/api_http_contracts.rs`.

## Dependencies / blocks

F04/F05/I01/I03/I07/R02.

## Acceptance criteria

Org context + permissions; stable errors; idempotent reindex;
HTTP contract/pagination/IDOR/malformed tests.

## Required tests / evidence

Org context + permissions; stable errors; idempotent reindex;
HTTP contract/pagination/IDOR/malformed tests.

## Security and migration notes

Bounded body/page, no internals.

## Out of scope

admin membership API.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Completion/evidence commits

- `30603158015`
- `91070008980`
- `b5cc92c`

- GitHub sync-closed timestamp: `2026-07-31T06:23:26Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
