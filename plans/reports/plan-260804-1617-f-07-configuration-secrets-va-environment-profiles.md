<!-- generated-done-issue-plan: F-07 -->
# F-07 — Configuration, secrets và environment profiles

Date: 2026-08-04
Source issue: [#52](https://github.com/anhnth24/project-example/issues/52)
Catalog: [`backlog/phase-f/issues/README.md`](../markhand-web/backlog/phase-f/issues/README.md)
Phase plan: [`phase-f-engineering-foundation.md`](../markhand-web/phase-f-engineering-foundation.md)
Status: Done

## Objective

Typed, fail-fast, secret-safe config cho local/test/prod.

## Context

- Phase: `F`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> merged to `master` via PR #171.

## Implementation plan

Define precedence; profile schema; mounted secret/env
references; validation/redacted Debug; `.env.example`; unsafe dev defaults isolated.

## Files/modules

`crates/server/src/config.rs`, `deploy/dev/.env.example`,
`docs/conventions/config-secrets.md`, config tests.

## Dependencies / blocks

F-02; blocks dev stack/server issues.

## Acceptance criteria

Missing/invalid config fails startup; no secret in errors;
prod cannot use dev credentials/profile.

## Required tests / evidence

Table/env/file precedence, redaction canary, profile deny.

## Security and migration notes

No committed secrets; rotation contract documented; N/A schema.

## Out of scope

Production secret-manager implementation.

## Delivery evidence

### Implementation PRs

- [PR #171](https://github.com/anhnth24/project-example/pull/171) — feat: add typed server configuration profiles; merged `2026-07-17T11:36:55Z`

### Recorded commit/SHA references

- `ddedd02c60c19e9a06d1f0d5222e93d1183f692a`

- GitHub sync-closed timestamp: `2026-07-17T12:42:20Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
