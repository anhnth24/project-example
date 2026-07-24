# P1B-O05 mixed-load soak / qualification

- Status: `incomplete`
- Issue: `P1B-O05`
- Canonical JSON: `o05-soak.json`
- Profile: `/workspace/bench/markhand_web/workloads/phase1b-mixed.yaml`
- Smoke non-qualifying: `True`
- Raw: `/workspace/bench/markhand_web/reports/phase-1b-gate/raw/o05-20260724T031602Z`

## Notes

Smoke/non-qualifying duration; cannot pass official O05.

## Blockers

- `smoke_non_qualifying_duration`
- `prerequisites_incomplete`
- `metrics_not_measured`
- `injection_or_recovery_failed`
- `gate:queryP95:unknown`
- `gate:queryP99:unknown`
- `gate:ingestThroughput:unknown`
- `gate:rssGrowth:unknown`
- `gate:tempGrowth:unknown`
- `gate:queueDepth:unknown`
- `gate:dbConnections:unknown`
- `gate:unboundedGrowth:unknown`
- `gate:recovery:unknown`
- `gate:postRestoreRetrieval:unknown`
- `gate:requestErrors:unknown`
- `gate:completeness:unknown`
- `arch:compare_dataset_unavailable`

## Gates

- `completeness`: `unknown`
- `dbConnections`: `unknown`
- `ingestThroughput`: `unknown`
- `postRestoreRetrieval`: `unknown`
- `queryP95`: `unknown`
- `queryP99`: `unknown`
- `queueDepth`: `unknown`
- `recovery`: `unknown`
- `requestErrors`: `unknown`
- `rssGrowth`: `unknown`
- `tempGrowth`: `unknown`
- `unboundedGrowth`: `unknown`
