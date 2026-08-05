<!-- generated-done-issue-plan: F-04 -->
# F-04 — TypeScript/React conventions

Issue closed: 2026-07-17
Source issue: [#49](https://github.com/anhnth24/project-example/issues/49)
Catalog: [`backlog/phase-f/issues/README.md`](../markhand-web/backlog/phase-f/issues/README.md)
Phase plan: [`phase-f-engineering-foundation.md`](../markhand-web/phase-f-engineering-foundation.md)
Status: Done

## Objective

Chuẩn strict TS, component/hook/state và accessibility cho web.

## Context

- Phase: `F`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> merged to `master` via PR #160.

## Implementation plan

TS strict policy; generated API immutable; naming/import
boundaries; state ownership; loading/error/empty; abort cleanup; a11y checklist.

## Files/modules

`web/tsconfig*.json`, ESLint/Prettier config,
`docs/conventions/typescript-react.md`, web test setup.

## Dependencies / blocks

F-02; blocks Phase 2 implementation.

## Acceptance criteria

No Tauri imports; generated code separated; hooks clean up
requests/streams; component patterns documented.

## Required tests / evidence

Typecheck/lint/format/unit sample/a11y smoke.

## Security and migration notes

No token/content logging or unsafe HTML by default; N/A schema.

## Out of scope

Full design system và Phase 2 pages.

## Delivery evidence

### Implementation PRs

- [PR #160](https://github.com/anhnth24/project-example/pull/160) — docs: establish Markhand Web architecture boundaries; merged `2026-07-17T08:08:21Z`

### Recorded commit/SHA references

- `ab3cc97f41a2021a8074500f808e220df9d54bbe`

- GitHub sync-closed timestamp: `2026-07-17T11:06:37Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
