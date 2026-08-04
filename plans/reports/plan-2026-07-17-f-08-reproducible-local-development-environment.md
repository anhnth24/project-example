<!-- generated-done-issue-plan: F-08 -->
# F-08 — Reproducible local development environment

Issue closed: 2026-07-17
Source issue: [#53](https://github.com/anhnth24/project-example/issues/53)
Catalog: [`backlog/phase-f/issues/README.md`](../markhand-web/backlog/phase-f/issues/README.md)
Phase plan: [`phase-f-engineering-foundation.md`](../markhand-web/phase-f-engineering-foundation.md)
Status: Done

## Objective

One-command CPU-only dev stack, optional GPU profile.

## Context

- Phase: `F`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> merged to `master` via PR #173.

## Implementation plan

Pin PG/Qdrant/MinIO/OTel; init buckets/extensions; health/
seed/reset; named volumes/private network; mock embedding; optional vLLM profile.

## Files/modules

`deploy/dev/compose.yml`, init/health/seed/reset scripts,
`docs/runbooks/local-development.md`.

## Dependencies / blocks

F-02/07; blocks Phase 0 spike and server development.

## Acceptance criteria

Clean machine up/health/seed/reset/down không console action;
restart preserves intended data; reset only dev resources.

## Required tests / evidence

CI compose smoke, service versions, cold setup transcript.

## Security and migration notes

Non-production credentials/private binds/no secret Git.

## Out of scope

Benchmark evidence và production orchestration.

## Delivery evidence

### Implementation PRs

- [PR #173](https://github.com/anhnth24/project-example/pull/173) — feat: add reproducible local development stack; merged `2026-07-17T12:41:30Z`

### Recorded commit/SHA references

- `58918f94d1ec5871765639e2569d8a01d01a3258`

- GitHub sync-closed timestamp: `2026-07-17T13:04:15Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
