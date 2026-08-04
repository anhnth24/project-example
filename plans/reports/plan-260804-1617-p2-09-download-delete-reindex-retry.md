<!-- generated-done-issue-plan: P2-09 -->
# P2-09 — Download/delete/reindex/retry

Date: 2026-08-04
Source issue: [#124](https://github.com/anhnth24/project-example/issues/124)
Catalog: [`backlog/phase-2/issues/README.md`](../markhand-web/backlog/phase-2/issues/README.md)
Phase plan: [`phase-2-web-spa.md`](../markhand-web/phase-2-web-spa.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Download/delete/reindex/retry**.

## Context

- Phase: `2`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> #312. Capability issue+redeem, delete tombstone có confirm. Không có endpoint retry-convert nên "thử lại" = reindex (ghi rõ trong UI).

## Implementation plan

Authorized actions, permission/confirm/conflict/idempotency handling.

## Files/modules

The source catalog records implementation and file scope together; see **Implementation plan** above.

## Dependencies / blocks

P2-07/08 + backend 1C guards.

## Acceptance criteria

Delete closes preview;
server deny wins; confirm/concurrency/stale/signed-route tests.

## Required tests / evidence

Delete closes preview;
server deny wins; confirm/concurrency/stale/signed-route tests.

## Security and migration notes

No client-built object URLs; CSRF/idempotency.

## Out of scope

purge policy.

## Delivery evidence

### Implementation PRs

- [PR #312](https://github.com/anhnth24/project-example/pull/312) — Web: Organic design system, left rail shell, and library wave 2 (P2-07/08/09); merged `2026-07-27T05:59:33Z`

### Recorded commit/SHA references

- `461417bc700811e5ebb251ff76caac11c13cc07c`

- GitHub sync-closed timestamp: `2026-07-27T09:49:17Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
