# ADR 0008: PG partition strategy for Markhand Web POC scale

- Status: Accepted
- Date: 2026-07-18
- Owners: storage-owner, architecture-owner
- Approver: Phase 0 architecture gate
- Decision key: `pg-partition-strategy`
- Related issues/PRs: P0-07; ADR 0001, 0006

## Context

The approved Phase 0 workload profile defines 20 orgs, 1M vectors/org and 20M
aggregate vectors, with Zipfian 80/20 tenant load and mixed query, ingest,
delete and recovery pressure. The current runner cannot run Docker or Profile B
services, and the existing spike environment report has `targetMatch=false`.

P0-07 therefore needs two separate outcomes:

1. choose a conservative Phase 1B POC topology that developers can implement
   without waiting for target hardware; and
2. keep production aggregate scale blocked until Profile B revalidation produces
   real Postgres/Qdrant measurements.

## Decision

For the Phase 1B single-org POC, use **no physical table partitioning** for the
primary Postgres metadata tables. Keep `org_id` mandatory in every API/query
path and preserve schemas/index definitions so bounded hash partitioning can be
introduced later without changing tenant semantics.

Reserve **bounded hash partitioning by `org_id` with 16 buckets** for
multi-tenant growth after Profile B revalidation. The reserved strategy is not
approved as production evidence by this ADR; it is the migration direction when
real aggregate load shows that no-partition tables no longer provide enough
delete, vacuum, index-maintenance or noisy-neighbor isolation.

## Consequences

- Positive: the Phase 1B POC remains simple, avoids premature partition routing,
  and keeps query/index behavior easy to validate.
- Positive: `org_id` remains the tenant boundary in SQL contracts, so later hash
  partitioning is an online schema/migration concern rather than an API change.
- Negative: this does not prove 20M aggregate mixed-load behavior and cannot be
  used to close production scale gates.
- Migration: before enabling bounded hash in production, run expand/copy or
  create-partition/backfill/cutover steps under the backup/restore runbook and
  compare query/delete/vacuum behavior on Profile B.

## Alternatives

- Start Phase 1B with hash partitions: rejected for the POC because the current
  evidence is synthetic and the POC is single-org; it would add operational
  complexity before the multi-tenant bottleneck is measured.
- Range/list partition by org: rejected for the reserved path because 20 orgs is
  not a stable production upper bound and per-org partitions create uneven
  hot-tenant maintenance.
- Partition by collection/document time: rejected because P0-07 load and
  deletion semantics are tenant-scoped, and every query must carry `org_id`.

## Verification

Current offline verification:

```bash
python3 bench/markhand_web/scripts/run_scale_topology.py --self-test
python3 bench/markhand_web/scripts/run_scale_topology.py
```

Inspect:

- `bench/markhand_web/scale/summary.json`
- `bench/markhand_web/reports/scale-topology.md`

The summary must record `targetMatch=false`, `productionScaleBlocked=true`, and
the explicit note that the harness does not claim `G0-SLO-QUERY-P99` / 20M
mixed-load evidence.

Production verification remains deferred to Profile B with real Postgres
metrics: filtered query P95/P99, insert/update/delete throughput, bloat/vacuum
behavior, index size, backup/restore timing and noisy-neighbor isolation.

## Exception lifecycle

This ADR permits a narrow Phase 1B POC exception: no-partition Postgres tables
may be used before production aggregate evidence exists.

- Owner: storage-owner
- Scope: Phase 1B POC and synthetic/de-identified benchmark data only
- Expiry: before any production aggregate-scale rollout
- Required revalidation: Profile B mixed-load run covering the approved P0-01
  workload profile, including 20M aggregate vectors and restore markers
- Exit condition: keep no-partition with measured headroom, or migrate to
  bounded hash partitions with a tested cutover/rollback plan
