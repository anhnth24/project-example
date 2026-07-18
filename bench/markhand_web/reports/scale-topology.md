# P0-07 scale topology offline report

- Generated: `2026-07-18T19:07:41.458289Z`
- Mode: `offline-synthetic-in-process`
- Seed: `20260718`
- Workload profile: `on-prem-reference-v1`
- Git commit: `df3771eb823ea11586afb70b1019e7e30502a9b5`
- Dirty at harness start: `False`
- `targetMatch`: `false`
- `productionScaleBlocked`: `true`
- `pocTopologySelected`: `true`
- `p0_07_closed`: `true`

## Scope

This harness is offline and synthetic. It compares topology shape in-process
because Docker/live Postgres/Qdrant are not available on this runner.

Explicit note: does NOT claim G0-SLO-QUERY-P99 / 20M evidence.

Production 20M aggregate scale remains blocked pending Profile B.

## Tenant distribution

- Strategy: `zipfian-80-20`
- Orgs: `20`
- Top tenant count: `4`
- Top tenant load share: `0.8`
- Aggregate vectors represented: `20000000`

## Mixed load

- Query operations: `1600`
- Ingest operations: `1200`
- Delete operations: `120`
- `org_id` filter applied to every operation: `true`

## Qdrant topology comparison

| topology | query p95 ms | query p99 ms | mixed p99 ms | org filter |
|---|---:|---:|---:|---|
| shared-collection | 55.964 | 56.902 | 56.679 | true |
| cohort-collections | 72.085 | 73.177 | 72.987 | true |

## PG topology comparison

| topology | query p95 ms | query p99 ms | delete p95 ms | org filter |
|---|---:|---:|---:|---|
| no-partition | 16.745 | 17.739 | 28.361 | true |
| bounded-hash-16 | 21.972 | 22.883 | 21.76 | true |

## Snapshot/restore markers

| topology | snapshot marker | snapshot ms | restore marker | restore ms |
|---|---|---:|---|---:|
| qdrant:shared-collection | `synthetic-snapshot-0ff22532984e` | 880.884 | `synthetic-restore-f8ea5b745449` | 1506.234 |
| qdrant:cohort-collections | `synthetic-snapshot-30ed1f95e6e6` | 1108.169 | `synthetic-restore-b30812fa1931` | 2010.989 |
| pg:no-partition | `synthetic-snapshot-d1869dbfd34d` | 296.058 | `synthetic-restore-764f99ba4a66` | 534.002 |
| pg:bounded-hash-16 | `synthetic-snapshot-85fa157db5af` | 302.926 | `synthetic-restore-54c0a311008d` | 591.702 |

## Recommendation

- Qdrant: shared collection with mandatory `org_id` filter.
- PG: no-partition for 1B single-org POC; bounded hash reserved for multi-tenant growth.
- Production 20M: blocked pending Profile B.

Decision keys:

- `pg-partition-strategy`
- `qdrant-topology`

## Closure

| field | value |
|---|---|
| `adrsAccepted` | `true` |
| `harnessCompleted` | `true` |
| `recommendationRecorded` | `true` |
| `gitClean` | `true` |
