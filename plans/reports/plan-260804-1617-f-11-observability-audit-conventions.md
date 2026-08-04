<!-- generated-done-issue-plan: F-11 -->
# F-11 — Observability/audit conventions

Date: 2026-08-04
Base commit: UNKNOWN — not recorded in the source catalog
Source issue: [#56](https://github.com/anhnth24/project-example/issues/56)
Catalog: [`backlog/phase-f/issues/README.md`](../markhand-web/backlog/phase-f/issues/README.md)
Phase plan: [`phase-f-engineering-foundation.md`](../markhand-web/phase-f-engineering-foundation.md)
Status: Done

## Objective

Correlation/metrics/log/audit schema ổn định trước business services.

## Context

- Phase: `F`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> Catalog records status as Done.

## Implementation plan

Field names; request/job/version/signature propagation;
metric units/cardinality; log allowlist/redaction; audit envelope; sample middleware.

## Files/modules

`docs/conventions/observability-audit.md`,
`crates/server/src/telemetry/`, sample tests/config.

## Dependencies / blocks

F-01/06/07/09; blocks 1B telemetry/business routes.

## Acceptance criteria

Synthetic in-memory request→job fixture chứng minh field
propagation/redaction; không thêm durable queue, business route hoặc persisted
audit trong Phase F; metric naming valid; seeded content/token/key absent.

## Required tests / evidence

Trace propagation, cardinality lint, redaction canaries,
audit fixture.

## Security and migration notes

No document/prompt/token/key/URL/PII; audit schema versioned.

## Out of scope

Production dashboards/SIEM.

## Delivery evidence

### Implementation PRs

- [PR #179](https://github.com/anhnth24/project-example/pull/179) — feat: define observability and audit contracts; merged `2026-07-17T14:12:01Z`

### Completion/evidence commits

- `928167ceec39142f91b9188add41d00b048f10a3`

- GitHub sync-closed timestamp: `2026-07-18T17:05:44Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
