<!-- generated-done-issue-plan: F-05 -->
# F-05 — SQL/data/migration conventions

Date: 2026-08-04
Source issue: [#50](https://github.com/anhnth24/project-example/issues/50)
Catalog: [`backlog/phase-f/issues/README.md`](../markhand-web/backlog/phase-f/issues/README.md)
Phase plan: [`phase-f-engineering-foundation.md`](../markhand-web/phase-f-engineering-foundation.md)
Status: Done

## Objective

Ngăn schema/tenant/migration conventions bị phát minh theo từng PR.

## Context

- Phase: `F`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> merged to `master` via PR #164.

## Implementation plan

Naming/types/time/UUID/FK/check/index; `org_id`; transaction/
locking/idempotency; immutable migration; expand/backfill/cutover/contract; rollback.

## Files/modules

`docs/conventions/sql-migrations.md`,
`crates/server/migrations/README.md`, migration test harness skeleton.

## Dependencies / blocks

F-01/02; blocks Phase 1B schema.

## Acceptance criteria

Example migration/repository query hợp conventions; policy
fresh/upgrade/mixed-version rõ.

## Required tests / evidence

Empty DB apply, migration checksum/immutability,
rollback-compat sample.

## Security and migration notes

Tenant predicate/RLS review checklist bắt buộc.

## Out of scope

Business tables và RLS decision.

## Delivery evidence

### Implementation PRs

- [PR #164](https://github.com/anhnth24/project-example/pull/164) — docs: establish SQL migration conventions; merged `2026-07-17T08:18:04Z`

### Recorded commit/SHA references

- `8d8b326c9420744c7b07a0b61ead1f034df72ad6`

- GitHub sync-closed timestamp: `2026-07-17T11:06:40Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
