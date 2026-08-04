<!-- generated-done-issue-plan: F-09 -->
# F-09 — Root task runner, quality tools và CI baseline

Date: 2026-08-04
Base commit: UNKNOWN — not recorded in the source catalog
Source issue: [#54](https://github.com/anhnth24/project-example/issues/54)
Catalog: [`backlog/phase-f/issues/README.md`](../markhand-web/backlog/phase-f/issues/README.md)
Phase plan: [`phase-f-engineering-foundation.md`](../markhand-web/phase-f-engineering-foundation.md)
Status: Done

## Objective

Cùng command local/CI cho format/lint/test/build/dev/migrate.

## Context

- Phase: `F`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> Catalog records status as Done.

## Implementation plan

Add `just`/equivalent root tasks theo test conventions
F-10; Rust/TS/SQL checks; dependency/license/security baseline cho cả `app/` và
`web/`; changed-path optimization nhưng giữ full required gate; pin/bootstrap host
tools và native Rust/Tauri prerequisites.

## Files/modules

`Justfile` hoặc task runner, CI workflows, tool configs,
`docs/conventions/ci.md`.

## Dependencies / blocks

F-03…08 + F-10; blocks all implementation PRs.

## Acceptance criteria

Documented commands identical local/CI; failures actionable;
desktop existing CI vẫn chạy.

## Required tests / evidence

Clean checkout full task, cache miss/hit, intentional
format/lint/test failure fixtures.

## Security and migration notes

Least-privilege CI, pinned actions/tools, no secret artifact.

## Out of scope

Production release workflow.

## Delivery evidence

### Implementation PRs

- [PR #177](https://github.com/anhnth24/project-example/pull/177) — ci: unify root quality and dependency gates; merged `2026-07-17T14:00:05Z`

### Completion/evidence commits

- `bf37b6160ac65bae7deeb7318c305263a04ad271`

- GitHub sync-closed timestamp: `2026-07-17T14:12:40Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
