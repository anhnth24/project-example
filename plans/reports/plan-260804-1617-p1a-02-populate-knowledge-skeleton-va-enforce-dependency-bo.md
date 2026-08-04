<!-- generated-done-issue-plan: P1A-02 -->
# P1A-02 — Populate knowledge skeleton và enforce dependency boundaries

Date: 2026-08-04
Source issue: [#69](https://github.com/anhnth24/project-example/issues/69)
Catalog: [`backlog/phase-1a/issues/README.md`](../markhand-web/backlog/phase-1a/issues/README.md)
Phase plan: [`phase-1a-knowledge-extraction.md`](../markhand-web/phase-1a-knowledge-extraction.md)
Status: Done

## Objective

Hoàn thiện skeleton `crates/knowledge` do F-02 tạo thành reusable
crate có typed errors và optional desktop features.

## Context

- Phase: `1A`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> merged to `master` via PR #186.

## Implementation plan

Populate modules types/embedding/query/rank/citation/ask; features
`desktop-sqlite`, `desktop-hnsw`; mở rộng CI deny-list theo boundary F-01. Không
tạo lại workspace member hoặc convention.

## Files/modules

`Cargo.toml`, `crates/knowledge/**`, `.github/workflows/ci.yml`.

## Dependencies / blocks

Baseline committed.

## Acceptance criteria

Build no-feature/all-feature; default tree không SQLite/HNSW; không
Tauri/axum/desktop; API không có DATA-root.

## Required tests / evidence

`cargo check/test/tree` feature matrix.

## Security and migration notes

Minimal dependency review.

## Out of scope

PG/Qdrant/server.

## Delivery evidence

### Implementation PRs

- [PR #186](https://github.com/anhnth24/project-example/pull/186) — feat: populate reusable knowledge crate skeleton; merged `2026-07-17T16:44:42Z`

### Recorded commit/SHA references

- `cf80285d97f6915d32c6c202cffbabc3c8cba3bb`

- GitHub sync-closed timestamp: `2026-07-18T17:06:08Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
