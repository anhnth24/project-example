<!-- generated-done-issue-plan: P2-04 -->
# P2-04 — Fetch-based SSE transport

Date: 2026-08-04
Base commit: UNKNOWN — not recorded in the source catalog
Source issue: [#119](https://github.com/anhnth24/project-example/issues/119)
Catalog: [`backlog/phase-2/issues/README.md`](../markhand-web/backlog/phase-2/issues/README.md)
Phase plan: [`phase-2-web-spa.md`](../markhand-web/phase-2-web-spa.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Fetch-based SSE transport**.

## Context

- Phase: `2`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> Catalog records status as Done.

## Implementation plan

Streaming parser với bearer, Last-Event-ID, dedupe/gap, refresh/
reconnect/backoff/snapshot, abort on scope change.

## Files/modules

The source catalog records implementation and file scope together; see **Implementation plan** above.

## Dependencies / blocks

P2-02/03.

## Acceptance criteria

Không native EventSource/token URL;
chunk boundary/reconnect/order/revoke/cancel tests.

## Required tests / evidence

Không native EventSource/token URL;
chunk boundary/reconnect/order/revoke/cancel tests.

## Security and migration notes

Bounded buffer/backoff, no content logs.

## Out of scope

WebSocket.

## Delivery evidence

### Implementation PRs

- [PR #311](https://github.com/anhnth24/project-example/pull/311) — Web wave 0 remainder and wave 1: client, SSE, mocks, login shell, scope-safe org switch; merged `2026-07-27T03:09:05Z`

### Completion/evidence commits

- `370c8f738af25f8becb4ecde709057b4ed70a8d4`

- GitHub sync-closed timestamp: `2026-07-27T09:48:57Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
