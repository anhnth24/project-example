<!-- generated-done-issue-plan: P1A-04 -->
# P1A-04 — Durable identities và index signatures

Date: 2026-08-04
Source issue: [#71](https://github.com/anhnth24/project-example/issues/71)
Catalog: [`backlog/phase-1a/issues/README.md`](../markhand-web/backlog/phase-1a/issues/README.md)
Phase plan: [`phase-1a-knowledge-extraction.md`](../markhand-web/phase-1a-knowledge-extraction.md)
Status: Done

## Objective

Deterministic server identities, desktop compatibility.

## Context

- Phase: `1A`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> merged to `master` via PR #189.

## Implementation plan

Versioned length-delimited encoding; BLAKE3/SHA-256 document/chunk/index;
signature model/revision/dim/normalize/chunk/text version; fixed vectors; legacy
`DefaultHasher` compatibility.

## Files/modules

`crates/knowledge/src/{identity,embedding}.rs`, identity fixtures.

## Dependencies / blocks

Shared metadata; production values tới từ Phase 0.

## Acceptance criteria

Cross-platform stable; no concatenation ambiguity; server không
dùng DefaultHasher; legacy index mở hoặc explicit rebuild.

## Required tests / evidence

Unicode/boundary/order/version/cross-process + legacy fixture.

## Security and migration notes

Hash là identity, không phải access control; không mix version.

## Out of scope

Chọn model.

## Delivery evidence

### Implementation PRs

- [PR #189](https://github.com/anhnth24/project-example/pull/189) — feat: add durable knowledge identities and signatures; merged `2026-07-17T16:57:53Z`

### Recorded commit/SHA references

- `84ff303802fd9e0cffce31d0383fd0e3d274a2b1`

- GitHub sync-closed timestamp: `2026-07-18T17:06:13Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
