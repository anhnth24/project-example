<!-- generated-done-issue-plan: P1B-F05 -->
# P1B-F05 — Password auth, rotating sessions và browser refresh transport

Issue closed: 2026-07-19
Source issue: [#82](https://github.com/anhnth24/project-example/issues/82)
Catalog: [`backlog/phase-1b/issues/README.md`](../markhand-web/backlog/phase-1b/issues/README.md)
Phase plan: [`phase-1b-single-org-poc.md`](../markhand-web/phase-1b-single-org-poc.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Password auth, rotating sessions và browser refresh transport**.

## Context

- Phase: `1B`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> Catalog records status as Done.

## Implementation plan

Argon2; pinned JWT issuer/audience/alg/KID; short access; hashed rotating
refresh family; provider interface; POC guards/audit; chốt transport theo auth ADR.
Nếu dùng browser cookie: issue/rotate/clear `HttpOnly Secure SameSite`, CSRF token
binding + Origin validation và OpenAPI cookie contract.

## Files/modules

`src/auth/{password,jwt,session,provider,permissions,middleware}.rs`,
`routes/auth.rs`.

## Dependencies / blocks

F03/F04 + auth ADR.

## Acceptance criteria

Login/refresh/logout/me; reuse revokes family; disabled user
blocked; alg/issuer/audience/expiry/race/permission/audit tests; cookie attributes,
CSRF missing/mismatch, cross-origin refresh/logout và cookie clearing tests nếu ADR
chọn cookie.

## Required tests / evidence

Login/refresh/logout/me; reuse revokes family; disabled user
blocked; alg/issuer/audience/expiry/race/permission/audit tests; cookie attributes,
CSRF missing/mismatch, cross-origin refresh/logout và cookie clearing tests nếu ADR
chọn cookie.

## Security and migration notes

No token/password logs.

## Out of scope

OIDC/MFA/recovery.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- UNKNOWN — no completion/evidence commit is cited in the catalog status.

- GitHub sync-closed timestamp: `2026-07-19T15:17:18Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
