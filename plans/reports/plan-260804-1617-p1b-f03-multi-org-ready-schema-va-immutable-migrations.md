<!-- generated-done-issue-plan: P1B-F03 -->
# P1B-F03 — Multi-org-ready schema và immutable migrations

Date: 2026-08-04
Source issue: [#80](https://github.com/anhnth24/project-example/issues/80)
Catalog: [`backlog/phase-1b/issues/README.md`](../markhand-web/backlog/phase-1b/issues/README.md)
Phase plan: [`phase-1b-single-org-poc.md`](../markhand-web/phase-1b-single-org-poc.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Multi-org-ready schema và immutable migrations**.

## Context

- Phase: `1B`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> Catalog records status as Done.

## Implementation plan

Migrations org/auth/RBAC/groups/collections, immutable versions/artifacts,
atomic current-published pointer, parent/version/effective lineage, chunks/FTS,
normalized claims, conflict/evidence lifecycle, jobs/outbox, quota/audit/index;
seed POC riêng.

## Files/modules

`crates/server/migrations/000*.sql`, `src/db/models.rs`.

## Dependencies / blocks

F01 + G0-ARCH.

## Acceptance criteria

Mọi business row có org; immutable versions; exactly one
current effective published version/logical document; concurrent publish/as-of/
lineage checks; fresh + supported-upgrade migration/schema introspection.

## Required tests / evidence

Mọi business row có org; immutable versions; exactly one
current effective published version/logical document; concurrent publish/as-of/
lineage checks; fresh + supported-upgrade migration/schema introspection.

## Security and migration notes

Files immutable sau merge; RLS theo ADR.

## Out of scope

custom role UI.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- UNKNOWN — no completion/evidence commit is cited in the catalog status.

- GitHub sync-closed timestamp: `2026-07-19T15:17:13Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
