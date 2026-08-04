<!-- generated-done-issue-plan: P2-03 -->
# P2-03 — Typed HTTP client/session refresh

Issue closed: 2026-07-27
Source issue: [#118](https://github.com/anhnth24/project-example/issues/118)
Catalog: [`backlog/phase-2/issues/README.md`](../markhand-web/backlog/phase-2/issues/README.md)
Phase plan: [`phase-2-web-spa.md`](../markhand-web/phase-2-web-spa.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Typed HTTP client/session refresh**.

## Context

- Phase: `2`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> #311. Refresh single-flight; ADR 0010 rotation; concurrent-401/revoke tests.

## Implementation plan

Fetch wrapper, access token memory, refresh single-flight, one retry,
normalized errors/request ID/quota, abort.

## Files/modules

The source catalog records implementation and file scope together; see **Implementation plan** above.

## Dependencies / blocks

P2-02.

## Acceptance criteria

Concurrent 401 một refresh; revoked refresh
logout; race/loop/malformed/403/429/network/abort tests.

## Required tests / evidence

Concurrent 401 một refresh; revoked refresh
logout; race/loop/malformed/403/429/network/abort tests.

## Security and migration notes

No token storage/log.

## Out of scope

offline queue/Tauri IPC.

## Delivery evidence

### Implementation PRs

- [PR #311](https://github.com/anhnth24/project-example/pull/311) — Web wave 0 remainder and wave 1: client, SSE, mocks, login shell, scope-safe org switch; merged `2026-07-27T03:09:05Z`

### Recorded commit/SHA references

- `370c8f738af25f8becb4ecde709057b4ed70a8d4`

- GitHub sync-closed timestamp: `2026-07-27T09:48:54Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
