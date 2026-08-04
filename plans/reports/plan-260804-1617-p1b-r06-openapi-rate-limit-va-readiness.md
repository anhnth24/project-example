<!-- generated-done-issue-plan: P1B-R06 -->
# P1B-R06 — OpenAPI, rate limit và readiness

Date: 2026-08-04
Base commit: UNKNOWN — not recorded in the source catalog
Source issue: [#96](https://github.com/anhnth24/project-example/issues/96)
Catalog: [`backlog/phase-1b/issues/README.md`](../markhand-web/backlog/phase-1b/issues/README.md)
Phase plan: [`phase-1b-single-org-poc.md`](../markhand-web/phase-1b-single-org-poc.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **OpenAPI, rate limit và readiness**.

## Context

- Phase: `1B`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> 24-core Ubuntu Docker host: `r06-hanging-soak.json` `status=pass`, 0 blockers,
> raw `r06-20260731T080518Z-eee30b03`. All four network readiness probes
> (`database`, `vector_store`, `object_store`, `embedding`) sustained 60s with
> correct 503 probe codes, bounded `/ready` deadlines, `/health/live` +
> `/openapi.yaml` within budget, bounded concurrent checkers, and confirmed
> restore/recovery. Hermetic router/readiness/unit coverage unchanged (Sol R2).
> Harness fix: post-pause `wait_for_hung_ready` excludes pool-drain transition
> samples before the sustain window (see `bench/markhand_web/hanging_soak/`).

## Implementation plan

Complete OpenAPI/fixtures; request IDs; CORS; IP auth/user limits; quota
metadata; live/ready/start checks.

## Files/modules

`api/openapi.rs`, OpenAPI YAML, `middleware/**`, `routes/health.rs`,
`routes/rate_limit_guard.rs`, `services/readiness.rs`.

## Dependencies / blocks

R04/R05/F05 + G0-SLO.

## Acceptance criteria

Every route represented two-way; readiness detects required
deps/signature/reconciliation with bounded deadlines; 429 metadata; trusted-proxy/
outage tests.

## Required tests / evidence

Every route represented two-way; readiness detects required
deps/signature/reconciliation with bounded deadlines; 429 metadata; trusted-proxy/
outage tests.

## Security and migration notes

Conservative CORS/proxy trust.

## Out of scope

distributed limiter.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Completion/evidence commits

- `20260731`
- `eee30b03`

- GitHub sync-closed timestamp: `2026-07-31T08:36:13Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
