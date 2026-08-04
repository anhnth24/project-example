<!-- generated-done-issue-plan: P1B-R01 -->
# P1B-R01 — Tenant-scoped hybrid retrieval

Issue closed: 2026-07-21
Source issue: [#91](https://github.com/anhnth24/project-example/issues/91)
Catalog: [`backlog/phase-1b/issues/README.md`](../markhand-web/backlog/phase-1b/issues/README.md)
Phase plan: [`phase-1b-single-org-poc.md`](../markhand-web/phase-1b-single-org-poc.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Tenant-scoped hybrid retrieval**.

## Context

- Phase: `1B`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> PR #252 + authorization hardening PR #254 merged; hermetic
> unit acceptance in `services/retrieval` and gated PG tests in `tests/retrieval.rs`.

## Implementation plan

Resolve scope + current/as-of/compare/history mode; query embed; parallel
Qdrant/FTS với version filter; knowledge merge/rerank; PG hydration/recheck
state/ACL/version; hydrate only conflict evidence whose both sides remain authorized.

## Files/modules

`services/retrieval/{mod,vector,fts,hydrate}.rs`, `db/search.rs`.

## Dependencies / blocks

F04/F06/I06 + G0-RET/G1A.

## Acceptance criteria

Empty scope deny; stale vector no text; current không trả
superseded version; as-of resolve đúng effective version; compare/history cùng
lineage; golden quality/cross-scope/deleted/one-leg outage/latency tests.

## Required tests / evidence

Empty scope deny; stale vector no text; current không trả
superseded version; as-of resolve đúng effective version; compare/history cùng
lineage; golden quality/cross-scope/deleted/one-leg outage/latency tests.

## Security and migration notes

Text only after authorized hydration.

## Out of scope

new reranker.

## Delivery evidence

### Implementation PRs

- [PR #252](https://github.com/anhnth24/project-example/pull/252) — feat(server): P1B-R01 tenant-scoped hybrid retrieval; merged `2026-07-21T13:12:25Z`
- [PR #254](https://github.com/anhnth24/project-example/pull/254) — fix(server): complete P1B-R01 authorization gates; merged `2026-07-21T15:35:28Z`

### Recorded commit/SHA references

- `6fbd417d00b9d9da545c230aed8ffd6b98f131d6`
- `749689600432bd8d80d5df38ab462a561a3ad5b0`

- GitHub sync-closed timestamp: `2026-07-21T13:12:26Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
