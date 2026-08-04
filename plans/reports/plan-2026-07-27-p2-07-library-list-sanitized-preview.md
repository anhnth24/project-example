<!-- generated-done-issue-plan: P2-07 -->
# P2-07 — Library/list/sanitized preview

Issue closed: 2026-07-27
Source issue: [#122](https://github.com/anhnth24/project-example/issues/122)
Catalog: [`backlog/phase-2/issues/README.md`](../markhand-web/backlog/phase-2/issues/README.md)
Phase plan: [`phase-2-web-spa.md`](../markhand-web/phase-2-web-spa.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Library/list/sanitized preview**.

## Context

- Phase: `2`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> #312. Collection nav, filter + cursor pagination thật, preview qua SafeMarkdown. Không có endpoint cross-collection nên "tất cả bộ sưu tập" chỉ điều hướng.

## Implementation plan

Adapt browser-safe LibraryView; collection navigation, filter/page,
status, preview states + SafeMarkdown; unresolved conflict badge/count, side-by-side
cited BA/design/dev claims và resolved-history link.

## Files/modules

The source catalog records implementation and file scope together; see **Implementation plan** above.

## Dependencies / blocks

P2-02/03/05/06.

## Acceptance criteria

Stable URL/pagination; API-only
preview; unsafe markdown, 403/404, switch-race tests. **+ ?doc= param**: select
pushes URL; deep-link preselects + previews; reload (fresh mount, same URL) keeps
selection; back/forward moves selection (`LibraryPage.test.tsx`'s "P2-07 URL param"
suite); E2E citation→preview (`qa.spec.ts`).

## Required tests / evidence

Stable URL/pagination; API-only
preview; unsafe markdown, 403/404, switch-race tests. **+ ?doc= param**: select
pushes URL; deep-link preselects + previews; reload (fresh mount, same URL) keeps
selection; back/forward moves selection (`LibraryPage.test.tsx`'s "P2-07 URL param"
suite); E2E citation→preview (`qa.spec.ts`).

## Security and migration notes

No local path/public key.

## Out of scope

desktop editor/compare.

## Delivery evidence

### Implementation PRs

- [PR #312](https://github.com/anhnth24/project-example/pull/312) — Web: Organic design system, left rail shell, and library wave 2 (P2-07/08/09); merged `2026-07-27T05:59:33Z`

### Recorded commit/SHA references

- `461417bc700811e5ebb251ff76caac11c13cc07c`

- GitHub sync-closed timestamp: `2026-07-27T09:49:09Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
