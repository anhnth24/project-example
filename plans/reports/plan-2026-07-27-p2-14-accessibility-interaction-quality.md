<!-- generated-done-issue-plan: P2-14 -->
# P2-14 — Accessibility/interaction quality

Issue closed: 2026-07-27
Source issue: [#129](https://github.com/anhnth24/project-example/issues/129)
Catalog: [`backlog/phase-2/issues/README.md`](../markhand-web/backlog/phase-2/issues/README.md)
Phase plan: [`phase-2-web-spa.md`](../markhand-web/phase-2-web-spa.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Accessibility/interaction quality**.

## Context

- Phase: `2`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> #313. axe không critical/serious (login/library/modal), focus-sau-route-change, progressbar job. Keyboard cho "ask" chưa làm được vì P2-10 chưa tồn tại.

## Implementation plan

Skip/landmark/focus/keyboard/progress labels/contrast/reduced motion.

## Files/modules

The source catalog records implementation and file scope together; see **Implementation plan** above.

## Dependencies / blocks

P2-05/07…12.

## Acceptance criteria

No axe critical; keyboard primary
flows; focus/reduced-motion/screen reader tests.

## Required tests / evidence

No axe critical; keyboard primary
flows; focus/reduced-motion/screen reader tests.

## Security and migration notes

Error không đọc internal/token.

## Out of scope

formal certification/i18n.

## Delivery evidence

### Implementation PRs

- [PR #313](https://github.com/anhnth24/project-example/pull/313) — Web accessibility pass and SPA static serving (P2-14, P2-16); merged `2026-07-27T08:32:24Z`

### Recorded commit/SHA references

- `85991a1bfbf156b48a1d7af68b0088880c866f7f`

- GitHub sync-closed timestamp: `2026-07-27T09:49:35Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
