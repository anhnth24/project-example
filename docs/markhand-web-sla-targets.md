# Markhand Web SLA/SLO targets for Phase 0 gate

Status: Phase 0 targets accepted for planning. Production exit remains blocked
until Profile B measurements run on `on-prem-reference` with `targetMatch=true`.

## Scope

These targets apply to the Markhand Web Phase 1B service envelope:

- 20 orgs, 10 collections/org, 5000 documents/collection, 10 pages/document;
- up to 1M vectors/org and 20M aggregate vectors;
- normal load of 20 concurrent queries and 300 ingested documents/hour;
- peak load of 80 concurrent queries and 1200 ingested documents/hour;
- recovery load of 2x normal for 120 minutes.

## Targets

| Area | Metric | Target | Gate | Current evidence | Profile B status |
|---|---:|---:|---|---|---|
| Retrieval latency | Query P95 | <= 500 ms | `G0-SLO-QUERY-P95` | No gate evidence yet; query load smoke only | Blocked |
| Retrieval latency | Filtered query P99 | <= 1000 ms | `G0-SLO-QUERY-P99` | P0-07 topology smoke has `targetMatch=false`; not a pass | Blocked |
| Retrieval quality | Recall@5 | >= 0.85 | `G0-RET-RECALL-AT-5` | `bench/markhand_web/retrieval/summary.json` | Phase 1B quality track accepted |
| Temporal answers | Temporal accuracy | >= 0.95 | `G0-RET-TEMPORAL-ACCURACY` | `bench/markhand_web/retrieval/summary.json` | Phase 1B quality track accepted |
| Change answers | Change accuracy | >= 0.95 | `G0-RET-CHANGE-ACCURACY` | `bench/markhand_web/retrieval/summary.json` | Phase 1B quality track accepted |
| Version citations | Precision/recall | 1.0 / 1.0 | `G0-RET-VERSION-CITATION-*` | `bench/markhand_web/retrieval/summary.json` | Phase 1B quality track accepted |
| Ingest throughput | Peak documents/hour | >= 1200 | `G0-CAP-INGEST-THROUGHPUT` | P0-08 local-cpu smoke has `targetMatch=false`; not a pass | Blocked |
| Queue age | Oldest ingest queue age under recovery load | <= 120 minutes and bounded | Capacity/recovery operational target | P0-08 deterministic simulation only | Blocked |
| DR RPO | Recovery point objective | <= 15 minutes | `G0-DR-RPO` | P0-10 restore smoke only; `targetMatch=false`; not a pass | Blocked |
| DR query-ready RTO | Query-ready recovery time | <= 60 minutes | `G0-DR-QUERY-READY-RTO` | P0-10 restore smoke only; `targetMatch=false`; not a pass | Blocked |
| DR full-vector RTO | Full vector rebuild/recovery time | <= 240 minutes | `G0-DR-FULL-VECTOR-RTO` | P0-10 restore smoke only; `targetMatch=false`; not a pass | Blocked |

## Measurement rules

- Gate-valid SLO, capacity and DR measurements require `environmentId` =
  `on-prem-reference` and `targetMatch=true`.
- Local/offline harnesses may close Phase 0 implementation smoke only when they
  set honest flags such as `targetMatch=false`, `productionScaleBlocked=true`,
  `productionCapacityBlocked=true` or `profileBDrBlocked=true`.
- G0-DR evidence remains null in `bench/markhand_web/gates.yaml` until a real
  component-loss restore drill runs on Profile B.
- Query-ready RTO means PostgreSQL is restored, MinIO consistency checks pass,
  reconciliation has run, authorization is enforced, and either a valid vector
  snapshot is restored or a documented text/FTS fallback is enabled.
- Full-vector RTO means the active index generation is restored or rebuilt and
  verified against ADR 0006/0011 signature rules.

## Profile B blockers

The following targets block production Phase 0 exit and any Phase 1B scale claim:

1. `G0-SLO-QUERY-P95` and `G0-SLO-QUERY-P99` live mixed query/ingest/delete run.
2. `G0-CAP-INGEST-THROUGHPUT` on the on-prem-reference worker profile.
3. `G0-DR-RPO`, `G0-DR-QUERY-READY-RTO` and `G0-DR-FULL-VECTOR-RTO` component-loss
   restore drill with real PostgreSQL, MinIO and Qdrant artifacts.
4. On-prem vLLM cutover evidence for production embedding runtime.
