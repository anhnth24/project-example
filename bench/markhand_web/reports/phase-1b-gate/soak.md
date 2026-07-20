# Phase-1B mixed-load soak

- Generated: `2026-07-20T00:44:24.926811Z`
- Mode: `self-skip-no-live-target`
- Status: `skipped`
- Profile: `phase-1b-soak-smoke`
- Git commit: `2395b2ba8c9249181d79529b86e4981d081d6331`
- Dirty at harness start: `true`
- `targetMatch`: `false`
- `targetResultsValidForGate`: `false`

## Caveat

Numeric G0-SLO/G0-CAP/soak gates require sustained real infrastructure. 
This sandbox does not provide that infrastructure; self-skip or targetMatch=false output is pending evidence only.

## Operation mix

- Duration seconds: `60`
- Ramp seconds: `10`
- Concurrency: `4`
- Operation weights: `{"ask": 20, "delete": 10, "ingest": 25, "query_search": 35, "reconcile": 10}`

| operation | attempts | ok | partial | error | skipped | p95 ms | p99 ms |
|---|---:|---:|---:|---:|---:|---:|---:|

## Monitored metrics

- Samples: `0`
- Ready failures: `0`
- Metrics scrape failures: `0`
- Families observed: `none`

Queue/leak proxy gauges:

| metric | samples | first | last | min | max | growth | monotonic |
|---|---:|---:|---:|---:|---:|---:|---|
| none | 0 |  |  |  |  |  |  |

Unavailable optional leak proxies:

- `markhand_process_resident_memory_bytes`
- `process_resident_memory_bytes`
- `markhand_temp_bytes`
- `markhand_open_connections`
- `markhand_jobs_dead_letter_total`
- `markhand_jobs_dead_letter_depth`

## Gate metric mapping

| gate id | soak metric |
|---|---|
| `G0-SLO-QUERY-P95` | `operations.query_search.durationMs.p95` |
| `G0-SLO-QUERY-P99` | `operations.query_search.durationMs.p99` |
| `G0-CAP-INGEST-THROUGHPUT` | `operations.ingest.successfulDocumentsPerHour` |
| `G0-DR-RPO` | `restore.rpoMinutes` |
| `G0-DR-QUERY-READY-RTO` | `restore.queryReadyRtoMinutes` |
| `G0-DR-FULL-VECTOR-RTO` | `restore.fullVectorRtoMinutes` |

## Failure-injection plan

| id | executed by harness | operator recorded executed | expected signal |
|---|---|---|---|
| `pause-convert-worker` | `false` | `false` | markhand_jobs_queue_depth remains bounded after worker recovery |
| `readiness-fence-reconciling` | `false` | `false` | /api/v1/health/ready reports not_reconciled until fence is ready |

## Notes

- No target URL/token was configured, so no live load was executed.
- Run against F02 compose.poc.yml or real on-prem-reference infrastructure to produce numeric evidence.
- does NOT claim numeric G0-SLO/G0-CAP/soak pass evidence without sustained real infra

Dirty paths at harness start:
- `bench/markhand_web/reports/phase-1b-gate.py`
- `bench/markhand_web/reports/phase-1b-gate/`
- `bench/markhand_web/soak/`
- `bench/markhand_web/workloads/`
