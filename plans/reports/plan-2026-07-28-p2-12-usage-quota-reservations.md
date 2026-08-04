<!-- generated-done-issue-plan: P2-12 -->
# P2-12 — Usage/quota/reservations

Issue closed: 2026-07-28
Source issue: [#127](https://github.com/anhnth24/project-example/issues/127)
Catalog: [`backlog/phase-2/issues/README.md`](../markhand-web/backlog/phase-2/issues/README.md)
Phase plan: [`phase-2-web-spa.md`](../markhand-web/phase-2-web-spa.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Usage/quota/reservations**.

## Context

- Phase: `2`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> #317. Usage cards từ `GET /usage` (endpoint tổng hợp landed cùng lát membership); route gate `member.manage`. Actionable 429 dùng chung path với document actions.

## Implementation plan

Usage cards, limits, active reservations/jobs, actionable 429.

## Files/modules

The source catalog records implementation and file scope together; see **Implementation plan** above.

## Dependencies / blocks

P2-03/05 + backend 1C-09…11.

## Acceptance criteria

API numbers match;
unit/timezone/403/429/stale tests.

## Required tests / evidence

API numbers match;
unit/timezone/403/429/stale tests.

## Security and migration notes

No client-derived authority/cross-org usage.

## Out of scope

billing.

## Delivery evidence

### Implementation PRs

- [PR #317](https://github.com/anhnth24/project-example/pull/317) — Membership admin, end to end: server API + web UI (P2-11, P2-12; 1C-02/1C-11 slice); merged `2026-07-28T06:34:53Z`

### Recorded commit/SHA references

- `64b80d47fecad7d28c4a2b2df2422a892d56e46b`

- GitHub sync-closed timestamp: `2026-07-28T07:50:05Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
