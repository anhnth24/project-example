# ADR 0009: Qdrant topology for Markhand Web POC scale

- Status: Accepted
- Date: 2026-07-18
- Owners: retrieval-owner, storage-owner, architecture-owner
- Approver: Phase 0 architecture gate
- Decision key: `qdrant-topology`
- Related issues/PRs: P0-07; ADR 0004, 0005, 0006, 0008

## Context

The approved workload profile targets 20 orgs, 10 collections/org, 1M
vectors/org and 20M aggregate vectors with Zipfian 80/20 tenant load. Every
retrieval request must be tenant-scoped. The current runner has no Docker/live
Qdrant available, and the existing spike evidence records `targetMatch=false`.

P0-07 can select a Phase 1B POC topology, but it cannot honestly claim Profile B
or 20M production mixed-load evidence from this environment.

## Decision

For the Phase 1B POC, use a **shared Qdrant collection** with a mandatory
`org_id` payload filter on every query, ingest mutation and delete operation.

The shared collection is the default implementation topology for POC work
because it keeps collection lifecycle, index signature handling and rebuild
flows simple while the product is still validating single-org and early
multi-tenant behavior.

Cohort collections remain a reserved production-scale option. Cohorts may be
introduced after Profile B revalidation if real measurements show that shared
collection filtering has insufficient tail-latency, compaction, snapshot,
restore or noisy-neighbor headroom at aggregate scale.

## Consequences

- Positive: one collection per index generation simplifies rebuilds and aligns
  with ADR 0006 index signature boundaries.
- Positive: tenant isolation is expressed as mandatory `org_id` filters rather
  than collection naming conventions.
- Positive: the POC can move forward without synthetic evidence being presented
  as a production SLO result.
- Negative: shared collection hot-tenant behavior, payload index selectivity and
  compaction effects still require live Qdrant measurement.
- Migration: moving to cohort collections creates a new deployment topology and
  requires dual-write/backfill/cutover or full rebuild per index generation.

## Alternatives

- Collection per org: rejected for the POC because 20 orgs is only the approved
  Phase 0 envelope and per-org collection lifecycle increases rebuild and
  migration complexity.
- Collection per org collection: rejected because 200 Qdrant collections for the
  approved profile would couple product collection count to storage topology and
  complicate global retrieval changes.
- Cohort collections immediately: rejected until Profile B evidence shows a
  measured need for noisy-neighbor isolation or operational split points.

## Verification

Current offline verification:

```bash
python3 bench/markhand_web/scripts/run_scale_topology.py --self-test
python3 bench/markhand_web/scripts/run_scale_topology.py
```

Inspect:

- `bench/markhand_web/scale/summary.json`
- `bench/markhand_web/reports/scale-topology.md`

The summary must record `pocTopologySelected=true`,
`productionScaleBlocked=true`, `targetMatch=false`, and the explicit note that
the harness does not claim `G0-SLO-QUERY-P99` / 20M mixed-load evidence.

Production verification remains deferred to Profile B with real Qdrant metrics:
filtered query P95/P99, recall under quantization/index settings, payload index
selectivity, RAM/disk, compaction behavior, snapshot/restore timing and
noisy-neighbor isolation.

## Exception lifecycle

This ADR permits a narrow Phase 1B POC exception: shared Qdrant collection may
be used before production aggregate evidence exists.

- Owner: retrieval-owner
- Scope: Phase 1B POC and synthetic/de-identified benchmark data only
- Expiry: before any production aggregate-scale rollout
- Required revalidation: Profile B mixed-load run covering the approved P0-01
  workload profile, including 20M aggregate vectors and restore markers
- Exit condition: keep shared collection with measured headroom, or migrate to
  cohort collections with tested rebuild, restore and rollback procedures
