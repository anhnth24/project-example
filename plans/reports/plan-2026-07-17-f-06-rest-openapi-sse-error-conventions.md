<!-- generated-done-issue-plan: F-06 -->
# F-06 — REST/OpenAPI/SSE/error conventions

Issue closed: 2026-07-17
Source issue: [#51](https://github.com/anhnth24/project-example/issues/51)
Catalog: [`backlog/phase-f/issues/README.md`](../markhand-web/backlog/phase-f/issues/README.md)
Phase plan: [`phase-f-engineering-foundation.md`](../markhand-web/phase-f-engineering-foundation.md)
Status: Done

## Objective

Contract thống nhất để backend/web không drift.

## Context

- Phase: `F`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> merged to `master` via PR #165.

## Implementation plan

`/api/v1`; resources/pagination/idempotency; canonical error;
date/UUID/enum/null; OpenAPI authority; SSE envelope/version/sequence/reconnect;
deprecation policy.

## Files/modules

`docs/conventions/api.md`, `crates/server/openapi/`,
sample DTO/error/SSE types và fixtures.

## Dependencies / blocks

F-01/02; blocks 1B routes và Phase 2 client.

## Acceptance criteria

Sample contract generate TS; error/SSE fixtures round-trip;
compatibility rules có examples.

## Required tests / evidence

OpenAPI validation/snapshot, Rust↔TS fixture,
SSE parser sequence sample.

## Security and migration notes

Errors không leak internal; SSE auth/revocation requirements;
persisted migration N/A.

## Out of scope

Business endpoints.

## Delivery evidence

### Implementation PRs

- [PR #165](https://github.com/anhnth24/project-example/pull/165) — feat: establish API and SSE contract conventions; merged `2026-07-17T10:13:29Z`

### Recorded commit/SHA references

- `599a63c30ecc3ba072f3f56a0a51e691c12de838`

- GitHub sync-closed timestamp: `2026-07-17T11:06:43Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
