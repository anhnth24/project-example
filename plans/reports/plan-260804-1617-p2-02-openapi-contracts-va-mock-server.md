<!-- generated-done-issue-plan: P2-02 -->
# P2-02 — OpenAPI contracts và mock server

Date: 2026-08-04
Base commit: UNKNOWN — not recorded in the source catalog
Source issue: [#117](https://github.com/anhnth24/project-example/issues/117)
Catalog: [`backlog/phase-2/issues/README.md`](../markhand-web/backlog/phase-2/issues/README.md)
Phase plan: [`phase-2-web-spa.md`](../markhand-web/phase-2-web-spa.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **OpenAPI contracts và mock server**.

## Context

- Phase: `2`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> Catalog records status as Done.

## Implementation plan

Pin generator; generated types; drift check; auth/org/library/job/
Q&A/admin/error/SSE fixtures và mock scenarios.

## Files/modules

The source catalog records implementation and file scope together; see **Implementation plan** above.

## Dependencies / blocks

Stable 1B OpenAPI.

## Acceptance criteria

Drift fails CI; generated files
immutable; fixture/schema/breaking-change tests; mock excluded production.

## Required tests / evidence

Drift fails CI; generated files
immutable; fixture/schema/breaking-change tests; mock excluded production.

## Security and migration notes

N/A — không thay đổi persisted schema; fixtures synthetic,
không chứa token/PII thật.

## Out of scope

Chờ toàn bộ 1C mới làm UI.

## Delivery evidence

### Implementation PRs

- [PR #311](https://github.com/anhnth24/project-example/pull/311) — Web wave 0 remainder and wave 1: client, SSE, mocks, login shell, scope-safe org switch; merged `2026-07-27T03:09:05Z`

### Completion/evidence commits

- `370c8f738af25f8becb4ecde709057b4ed70a8d4`

- GitHub sync-closed timestamp: `2026-07-27T09:48:50Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
