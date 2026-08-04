<!-- generated-done-issue-plan: F-10 -->
# F-10 — Test pyramid, fixtures và golden-data conventions

Issue closed: 2026-07-17
Source issue: [#55](https://github.com/anhnth24/project-example/issues/55)
Catalog: [`backlog/phase-f/issues/README.md`](../markhand-web/backlog/phase-f/issues/README.md)
Phase plan: [`phase-f-engineering-foundation.md`](../markhand-web/phase-f-engineering-foundation.md)
Status: Done

## Objective

Chuẩn test/evidence dùng chung trước Phase 0/1A.

## Context

- Phase: `F`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> merged to `master` via PR #175.

## Implementation plan

Define unit/integration/contract/denial/E2E/benchmark/
migration/restore layers; fixture IDs/time/checksum/license; CI artifact retention.

## Files/modules

`docs/conventions/testing-fixtures.md`, `tests/fixtures/README.md`,
sample fixture validators.

## Dependencies / blocks

F-03…08; blocks F-09, P0-02 và P1A-01.

## Acceptance criteria

Mỗi layer có owner/location/command; fixture synthetic/
deterministic; large artifacts policy rõ.

## Required tests / evidence

Fixture validator catches checksum, absolute path,
secret canary, duplicate ID.

## Security and migration notes

De-identification/license required; migration evidence format.

## Out of scope

Viết toàn bộ golden corpus.

## Delivery evidence

### Implementation PRs

- [PR #175](https://github.com/anhnth24/project-example/pull/175) — test: establish fixture and evidence conventions; merged `2026-07-17T13:35:19Z`

### Recorded commit/SHA references

- `33a22d929d81f6abf6fb6c58f30dd1c78b0a2e5a`

- GitHub sync-closed timestamp: `2026-07-17T14:12:43Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
