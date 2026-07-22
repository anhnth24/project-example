# API latency burn

## Detect
- `MarkhandApiLatencyBurn` on p95.

## Contain
- Shed non-critical load (reindex bursts).
- Confirm embedding/Qdrant legs are not both timing out.

## Recover
1. Inspect retrieval leg metrics (`markhand_retrieval_leg_duration_seconds`).
2. Scale read replicas / workers as capacity ADR allows.
3. Keep one-leg degradation path enabled.

## Verify
- p95 returns under SLO; no secret-bearing spans in traces.
