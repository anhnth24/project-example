<!-- generated-done-issue-plan: P2-05 -->
# P2-05 — Login/session/application shell

Date: 2026-08-04
Source issue: [#120](https://github.com/anhnth24/project-example/issues/120)
Catalog: [`backlog/phase-2/issues/README.md`](../markhand-web/backlog/phase-2/issues/README.md)
Phase plan: [`phase-2-web-spa.md`](../markhand-web/phase-2-web-spa.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Login/session/application shell**.

## Context

- Phase: `2`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> #311. Bearer refresh trong memory (không cookie/CSRF); router + guard matrix.

## Implementation plan

Router, auth bootstrap/login/protected shell/guards/logout/help stub.

## Files/modules

The source catalog records implementation and file scope together; see **Implementation plan** above.

## Dependencies / blocks

P2-01/03 + P1B-F05 browser refresh contract.

## Acceptance criteria

Intended route, expiry, guard matrix, login/refresh/logout component tests và
integration CSRF/cookie-origin contract theo auth ADR.

## Required tests / evidence

Intended route, expiry, guard matrix, login/refresh/logout component tests và
integration CSRF/cookie-origin contract theo auth ADR.

## Security and migration notes

Transport theo auth ADR. Nếu chọn cookie: HttpOnly/Secure/SameSite +
CSRF/Origin contract; nếu chọn bearer refresh: không cookie/CSRF nhưng token không
được persist/log. Server luôn là authority.

## Out of scope

signup/reset/MFA/OIDC.

## Delivery evidence

### Implementation PRs

- [PR #311](https://github.com/anhnth24/project-example/pull/311) — Web wave 0 remainder and wave 1: client, SSE, mocks, login shell, scope-safe org switch; merged `2026-07-27T03:09:05Z`

### Recorded commit/SHA references

- `370c8f738af25f8becb4ecde709057b4ed70a8d4`

- GitHub sync-closed timestamp: `2026-07-27T09:49:01Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
