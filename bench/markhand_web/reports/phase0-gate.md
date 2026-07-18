# P0-10 Phase 0 gate

- Generated: `2026-07-18T19:43:43.996239Z`
- Git commit: `54468d9190988ae738f8238bfe9197cc8f81a4f9`
- Git clean at harness start: `true`
- `p0_10_closed`: `true`
- `productionPhase0ExitBlocked`: `true`

## Checker results

| check | pass | stdout | stderr |
|---|---|---|---|
| `phase0Decisions` | `true` | `{"metric": "approved_architecture_decisions", "value": 7}` | `` |
| `markhandGates` | `true` | `Markhand workload and gate registry valid` | `` |
| `runtimeLicenseInventory` | `true` | `{"metric": "approved_runtime_licenses", "value": 1.0}` | `` |

## Smoke results

| smoke | pass | path | notes |
|---|---|---|---|
| `security` | `true` | `bench/markhand_web/security/summary.json` | p0_09_closed=true |
| `restore` | `true` | `bench/markhand_web/restore/summary.json` | p0_10_restore_smoke_closed=true; targetMatch=false; profileBDrGatePassed=false |
| `queryLoad` | `true` | `bench/markhand_web/query_load/summary.json` | p0_10_query_load_smoke_closed=true; targetMatch=false; profileBSloGatePassed=false |

## Closure flags

| flag | value |
|---|---|
| `decisionsAccepted` | `true` |
| `gateRegistryValid` | `true` |
| `runtimeLicenseInventoryPassed` | `true` |
| `securitySmokeClosed` | `true` |
| `restoreSmokeClosed` | `true` |
| `queryLoadSmokeClosed` | `true` |
| `gitClean` | `true` |
| `p0_10_closed` | `true` |
| `productionPhase0ExitBlocked` | `true` |

## Remaining Profile B blockers

| gate/item | owner | reason |
|---|---|---|
| `G0-SLO-QUERY-P95` | `operations-owner` | Query P95 requires live mixed-load measurement on on-prem-reference. |
| `G0-SLO-QUERY-P99` | `operations-owner` | Filtered query P99 requires live 20M aggregate vector and tenant-filter measurement. |
| `G0-CAP-INGEST-THROUGHPUT` | `worker-owner` | Ingest throughput/headroom remains local-cpu smoke until Profile B run. |
| `G0-DR-RPO` | `operations-owner` | RPO requires real component-loss backup/restore evidence. |
| `G0-DR-QUERY-READY-RTO` | `operations-owner` | Query-ready RTO requires live PG/MinIO/Qdrant restore drill. |
| `G0-DR-FULL-VECTOR-RTO` | `operations-owner` | Full-vector RTO requires live snapshot/rebuild timing on target hardware. |
| `G0-RET-VLLM-CUTOVER` | `retrieval-owner` | Production embedding cutover requires on-prem vLLM evidence. |
| `SLA-TTFT-P95` | `operations-owner` | Time-to-first-token P95 under normal load is not measured on Profile B. |
| `SLA-AVAILABILITY` | `operations-owner` | Monthly query-path availability is not measured on Profile B. |
| `SLA-DEGRADED-MODE` | `operations-owner` | Authz-safe FTS/text fallback under vector outage is not Profile B proven. |
