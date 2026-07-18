# ADR 0010: Auth session lifecycle for Phase 1B

- Status: Accepted
- Date: 2026-07-18
- Decision key: `auth-session-lifecycle`
- Owners: security-owner, server-owner, architecture-owner
- Approver: Phase 0 architecture gate
- Related issues/PRs: P0-10; ADR 0001; ADR 0007; Phase 1B single-org POC

## Context

Phase 1B needs a secure single-org vertical slice with login, upload, indexing
and retrieval. The POC must not hard-code a demo bypass that later blocks
multi-org tenancy, audit or revocation. `phase-1b-single-org-poc.md` requires
Argon2, JWT, short access tokens, rotating refresh tokens and an `OrgContext`
for every business path.

## Decision

Use first-party password auth for the Phase 1B POC:

- Passwords are hashed with Argon2id using a maintained library. Parameters are
  pinned in configuration and can be raised by a rehash-on-login path.
- Access tokens are signed JWTs with pinned algorithm, issuer, audience,
  subject, `kid`, `iat`, `nbf`, expiry and a session/family identifier. Access
  tokens are short lived; the default target is 15 minutes or less.
- Refresh tokens are opaque high-entropy secrets. Only their hash is stored in
  PostgreSQL, scoped to a token family, user, org, device/session metadata and
  expiry.
- Refresh tokens rotate on every use. Reuse of an already-rotated token revokes
  the whole family and emits an audit/security event.
- Logout revokes the active refresh token or family. Password reset and account
  disable revoke all active token families for that user.
- Every authenticated request resolves an `OrgContext` from current membership,
  permissions and allowed collections. JWT claims are hints only; current
  authorization is loaded from PostgreSQL before business reads/writes.
- OIDC/SSO is deferred behind an auth provider interface. It may mint the same
  internal session shape later, but it cannot bypass refresh rotation,
  `OrgContext` resolution or audit.

Tokens and passwords are never logged. Audit records include success/failure,
user/org/session identifiers, request ID and coarse client metadata, but not
secret material.

## Consequences

- Positive: Phase 1B has real revocation semantics instead of bearer tokens that
  remain valid until expiry of a long-lived secret.
- Positive: later OIDC can reuse the session and `OrgContext` contracts.
- Negative: refresh token storage and family-reuse detection add schema and
  test complexity to the POC.
- Security: authorization is current-state based, so removed collection access
  takes effect on the next request even if an access token still exists.

## Alternatives considered

- Long-lived JWT-only sessions: rejected because revocation and org membership
  changes would not take effect promptly.
- Server-side cookie-only sessions: viable later, but the POC needs a clear API
  token contract for web/SSE clients and workers.
- OIDC first: deferred to Phase 4 because Phase 1B needs a controlled POC auth
  path without external identity-provider dependency.

## Verification

Phase 1B implementation must include:

```bash
cargo test -p fileconv-server auth
cargo test -p fileconv-server tenant
```

Required cases:

- Argon2id hashes verify and rehash when parameters change;
- access token expiry, issuer, audience and `kid` are enforced;
- refresh token rotation invalidates the previous token;
- refresh-token reuse revokes the family;
- disabled users and removed org memberships cannot resolve `OrgContext`;
- login/logout/refresh failures are audited without logging secrets.

P0-10 accepts the lifecycle decision only. It does not claim a shipped auth
implementation.

## Exception lifecycle

N/A. Demo bypasses are not accepted for the Phase 1B server boundary.
