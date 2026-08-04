<!-- generated-done-issue-plan: P1B-I07 -->
# P1B-I07 — Tombstone delete và reconcile

Issue closed: 2026-07-21
Source issue: [#90](https://github.com/anhnth24/project-example/issues/90)
Catalog: [`backlog/phase-1b/issues/README.md`](../markhand-web/backlog/phase-1b/issues/README.md)
Phase plan: [`phase-1b-single-org-poc.md`](../markhand-web/phase-1b-single-org-poc.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Tombstone delete và reconcile**.

## Context

- Phase: `1B`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> merged via PR #245; #282 fixed reconcile audit `request_id`
> length so `live_reconcile_repairs_orphan_vectors` /
> `live_reconcile_dead_letter_staging_gc` pass under rust-integration. ADR 0015
> (purge retention semantics) remains Proposed — wording follow-up only, not a
> blocker for the delete/reconcile acceptance matrix already covered by live tests.

## Implementation plan

PG tombstone first; idempotent vector/object cleanup; dry-run/repair
missing/orphan/stale across three stores.

## Files/modules

`workers/{delete,reconcile}.rs`, `services/{deletion,reconciliation}.rs`.

## Dependencies / blocks

I03/I06/F06 + recovery ADR.

## Acceptance criteria

Immediate read suppression; drift safely repaired; repeated
repair, race, kill/resume matrix.

## Required tests / evidence

Immediate read suppression; drift safely repaired; repeated
repair, race, kill/resume matrix.

## Security and migration notes

Scoped destructive audit.

## Out of scope

legal hold/full ACL revoke.

## Delivery evidence

### Implementation PRs

- [PR #245](https://github.com/anhnth24/project-example/pull/245) — feat(server): P1B-I07 tombstone delete and reconcile; merged `2026-07-20T13:20:36Z`
- [PR #282](https://github.com/anhnth24/project-example/pull/282) — fix(server): reconcile audit request_id vượt giới hạn 64 ký tự (rust-integration đỏ); merged `2026-07-23T00:52:36Z`

### Recorded commit/SHA references

- `633c96de246754ee7624e9cb92a376585d5cd8d4`
- `e0fe30b2a1b27f0de6c6fb076718003198566964`

- GitHub sync-closed timestamp: `2026-07-21T03:40:37Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
