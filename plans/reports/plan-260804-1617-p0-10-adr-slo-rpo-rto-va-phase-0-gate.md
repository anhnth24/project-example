<!-- generated-done-issue-plan: P0-10 -->
# P0-10 — ADR, SLO/RPO/RTO và Phase 0 gate

Date: 2026-08-04
Base commit: UNKNOWN — not recorded in the source catalog
Source issue: [#67](https://github.com/anhnth24/project-example/issues/67)
Catalog: [`backlog/phase-0/issues/README.md`](../markhand-web/backlog/phase-0/issues/README.md)
Phase plan: [`phase-0-discovery-and-gates.md`](../markhand-web/phase-0-discovery-and-gates.md)
Status: Done

## Objective

Chuyển evidence thành quyết định và restore/query-load smoke proof.

## Context

- Phase: `0`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> (`phase0-decisions.json`); SLA/risk register + restore/query-load smoke recorded.
> Not a production Phase 0 numeric exit: Profile B gates (query P95/P99, ingest
> capacity, DR RPO/RTO, vLLM cutover) remain open (`productionPhase0ExitBlocked=true`).

## Implementation plan

ADR document/artifact, tenancy/RLS, partition, Qdrant, auth/session,
index migration, backup order; chốt SLO; offline restore smoke; close decision
registry with Profile B blockers listed.

## Files/modules

`docs/adr/`, `docs/markhand-web-{sla-targets,risk-register}.md`,
`bench/markhand_web/reports/restore-drill.md`, `phase0/summary.json`.

## Dependencies / blocks

P0-01…P0-09 + approvers.

## Acceptance criteria

Mọi decision được duyệt; restore/query-load
smoke honest (`targetMatch=false`); license + security smoke pass; risk register
có disposition; `productionPhase0ExitBlocked=true` khi Profile B gates còn mở.
clean restore đạt RPO/RTO
và mixed-load/capacity gates trên `on-prem-reference` với `targetMatch=true`.

## Required tests / evidence

`check-phase0-decisions.py`; phase0 gate harness; offline
restore/query-load smoke; Profile B component-loss restore remaining.

## Security and migration notes

PG authority; MinIO originals không reconstruct được;
migration expand/cutover/contract.

## Out of scope

Production HA và user onboarding.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Completion/evidence commits

- UNKNOWN — no completion/evidence commit is cited in the catalog status.

- GitHub sync-closed timestamp: `2026-07-18T19:45:13Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
