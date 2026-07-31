# P1B-R06 hanging-dependency Compose soak

- Status: `pass`
- Issue: `P1B-R06`
- Canonical JSON: `r06-hanging-soak.json`
- Raw: `bench/markhand_web/reports/phase-1b-gate/raw/r06-20260731T080518Z-eee30b03`

## Blockers

- (none)

## Dependencies

- `database` (`postgres`): gates={'readyCodeCorrect': 'pass', 'readyBounded': 'pass', 'liveBounded': 'pass', 'openapiBounded': 'pass', 'concurrencyBounded': 'pass', 'concurrencyNoGrowth': 'pass', 'restoreConfirmed': 'pass', 'recoveryBounded': 'pass'}
- `vector_store` (`qdrant`): gates={'readyCodeCorrect': 'pass', 'readyBounded': 'pass', 'liveBounded': 'pass', 'openapiBounded': 'pass', 'concurrencyBounded': 'pass', 'concurrencyNoGrowth': 'pass', 'restoreConfirmed': 'pass', 'recoveryBounded': 'pass'}
- `object_store` (`minio`): gates={'readyCodeCorrect': 'pass', 'readyBounded': 'pass', 'liveBounded': 'pass', 'openapiBounded': 'pass', 'concurrencyBounded': 'pass', 'concurrencyNoGrowth': 'pass', 'restoreConfirmed': 'pass', 'recoveryBounded': 'pass'}
- `embedding` (`mock-embedding`): gates={'readyCodeCorrect': 'pass', 'readyBounded': 'pass', 'liveBounded': 'pass', 'openapiBounded': 'pass', 'concurrencyBounded': 'pass', 'concurrencyNoGrowth': 'pass', 'restoreConfirmed': 'pass', 'recoveryBounded': 'pass'}
