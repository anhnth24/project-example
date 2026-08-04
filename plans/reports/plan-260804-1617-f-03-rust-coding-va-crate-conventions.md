<!-- generated-done-issue-plan: F-03 -->
# F-03 — Rust coding và crate conventions

Date: 2026-08-04
Source issue: [#48](https://github.com/anhnth24/project-example/issues/48)
Catalog: [`backlog/phase-f/issues/README.md`](../markhand-web/backlog/phase-f/issues/README.md)
Phase plan: [`phase-f-engineering-foundation.md`](../markhand-web/phase-f-engineering-foundation.md)
Status: Done

## Objective

Một chuẩn Rust bắt buộc cho core/knowledge/server/workers.

## Context

- Phase: `F`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> merged to `master` via PR #160.

## Implementation plan

Rustfmt/clippy policy; error/context; async vs blocking;
cancellation/timeouts; panic/unwrap/unsafe/public docs; naming/module visibility.

## Files/modules

`rustfmt.toml`, `clippy.toml` nếu cần,
`docs/conventions/rust.md`, root lint task, CI.

## Dependencies / blocks

F-02; blocks Rust feature issues.

## Acceptance criteria

Convention có enforceable rule + justified exceptions;
existing code có migration plan thay vì bật deny phá toàn repo ngay.

## Required tests / evidence

Format check, clippy selected warnings-as-errors,
forbidden-pattern baseline/delta.

## Security and migration notes

Request/worker path không panic; secret-safe errors; N/A schema.

## Out of scope

Refactor toàn bộ warning cũ trong cùng issue.

## Delivery evidence

### Implementation PRs

- [PR #160](https://github.com/anhnth24/project-example/pull/160) — docs: establish Markhand Web architecture boundaries; merged `2026-07-17T08:08:21Z`

### Recorded commit/SHA references

- `ab3cc97f41a2021a8074500f808e220df9d54bbe`

- GitHub sync-closed timestamp: `2026-07-17T11:06:34Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
