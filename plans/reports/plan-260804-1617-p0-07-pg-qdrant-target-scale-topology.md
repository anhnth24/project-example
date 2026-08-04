<!-- generated-done-issue-plan: P0-07 -->
# P0-07 — PG/Qdrant target-scale topology

Date: 2026-08-04
Base commit: UNKNOWN — not recorded in the source catalog
Source issue: [#64](https://github.com/anhnth24/project-example/issues/64)
Catalog: [`backlog/phase-0/issues/README.md`](../markhand-web/backlog/phase-0/issues/README.md)
Phase plan: [`phase-0-discovery-and-gates.md`](../markhand-web/phase-0-discovery-and-gates.md)
Status: Done

## Objective

Chọn Qdrant topology và PG partition strategy bằng mixed-load evidence.

## Context

- Phase: `0`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> Qdrant shared collection with mandatory `org_id` filter and PG no-partition
> for the single-org POC. Profile B `G0-SLO-QUERY-P99` / 20M mixed-load
> evidence still blocks production aggregate scale.

## Implementation plan

Generate realistic tenants; compare shared/cohort collection and
PG no-partition/bounded hash offline with query+ingest+delete+snapshot
markers. Re-run live on Profile B before production aggregate scale.

## Files/modules

`bench/markhand_web/scale/`, `reports/scale-topology.md`, ADR Qdrant/PG.

## Dependencies / blocks

POC decision unblocked; production aggregate scale remains
blocked by hardware/storage scale thật. Không chấp nhận offline extrapolation as
Profile B evidence.

## Acceptance criteria

1B POC recommendation recorded in ADR 0008/0009 and
`bench/markhand_web/scale/summary.json`; `productionScaleBlocked=true` until
Profile B validates filtered P95/P99/recall and restore behavior.

## Required tests / evidence

Offline harness self-test + full run. Deferred production
evidence: latency, throughput, quantized recall, RAM/disk, compaction,
noisy-neighbor, FTS on Profile B.

## Security and migration notes

Synthetic tenant; mọi query vẫn có tenant filter.

## Out of scope

Production RLS.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Completion/evidence commits

- UNKNOWN — no completion/evidence commit is cited in the catalog status.

- GitHub sync-closed timestamp: `2026-07-18T19:11:09Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
