# P0-10 query-load smoke

- Generated: `2026-07-18T19:37:30.692264Z`
- Mode: `offline-synthetic-query-load-smoke`
- Git commit: `eabb2e18cceb8cac925a6f4b4d7ab8e3c650a5d8`
- Dirty at harness start: `true`
- `targetMatch`: `false`
- `profileBSloGatePassed`: `false`
- `productionSloBlocked`: `true`
- `p0_10_query_load_smoke_closed`: `true`

## Scope

This is a deterministic smoke stub for the query-load command shape. It
does not run live PostgreSQL/Qdrant or 20M aggregate vector load.

Explicit note: does NOT claim G0-SLO-QUERY-P95 or G0-SLO-QUERY-P99 pass evidence.

## Synthetic metrics

| metric | synthetic value ms | target ms | synthetic within target | gate-valid pass |
|---|---:|---:|---|---|
| Query P95 | 296.008 | 500 | true | false |
| Filtered query P99 | 820.854 | 1000 | true | false |

Profile B requirement: run against `on-prem-reference` with `targetMatch=true`,
approved workload scale, live services and mixed query/ingest/delete pressure.

## Closure

| field | value |
|---|---|
| `stubExecuted` | `true` |
| `smokeMetricsEmitted` | `true` |
| `honestFlagsSet` | `true` |
| `profileBRequirementDocumented` | `true` |

Dirty paths at harness start:
- `docs/adr/0002-version-aware-citations.md`
- `docs/adr/README.md`
- `plans/markhand-web/backlog/github-issues.json`
- `bench/markhand_web/reports/restore-drill.md`
- `bench/markhand_web/restore/`
- `bench/markhand_web/scripts/run_phase0_gate.py`
- `bench/markhand_web/scripts/run_query_load.py`
- `bench/markhand_web/scripts/run_restore_drill.py`
- `docs/adr/0007-tenant-isolation-rls.md`
- `docs/adr/0010-auth-session-lifecycle.md`
- `docs/adr/0011-model-index-migration.md`
- `docs/adr/0012-backup-recovery-order.md`
- `docs/adr/phase0-decisions.json`
- `docs/markhand-web-risk-register.md`
- `docs/markhand-web-sla-targets.md`
