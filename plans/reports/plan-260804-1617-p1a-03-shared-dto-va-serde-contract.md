<!-- generated-done-issue-plan: P1A-03 -->
# P1A-03 — Shared DTO và serde contract

Date: 2026-08-04
Base commit: UNKNOWN — not recorded in the source catalog
Source issue: [#70](https://github.com/anhnth24/project-example/issues/70)
Catalog: [`backlog/phase-1a/issues/README.md`](../markhand-web/backlog/phase-1a/issues/README.md)
Phase plan: [`phase-1a-knowledge-extraction.md`](../markhand-web/phase-1a-knowledge-extraction.md)
Status: Done

## Objective

Di chuyển index/search/ask types mà không đổi JSON.

## Context

- Phase: `1A`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> Catalog records status as Done.

## Implementation plan

Index request/result/stats, hit/anchor/grounded answer/metadata; serde
fixtures; temporary desktop re-export.

## Files/modules

`crates/knowledge/src/types.rs`, serde fixtures/tests,
`app/src/lib/types.ts`.

## Dependencies / blocks

Scaffold + frozen JSON.

## Acceptance criteria

Canonical JSON equivalent; no desktop path/state type; TypeScript
không cần behavior change.

## Required tests / evidence

Rust round-trip + TS fixture tests.

## Security and migration notes

Errors không expose provider secrets.

## Out of scope

OpenAPI generation.

## Delivery evidence

### Implementation PRs

- [PR #188](https://github.com/anhnth24/project-example/pull/188) — feat: move knowledge DTO contracts to shared crate; merged `2026-07-17T16:50:41Z`

### Completion/evidence commits

- `fdb3b9a542ae3a34254c5dc904172f91ea1ccdbd`

- GitHub sync-closed timestamp: `2026-07-18T17:06:11Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
