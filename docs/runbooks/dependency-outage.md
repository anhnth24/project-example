# Runbook: Dependency outage (PG / Qdrant / MinIO / embedding / ready)

Issue: P1B-O02  
Alerts: `MarkhandDependencyProbeDown`, `MarkhandEmbeddingErrorOutbreak`,
`MarkhandRetrievalErrorOutbreak`, `MarkhandQueryLatencyP95Burn`,
`MarkhandQueryLatencyP99Burn`, `MarkhandAvailabilityBurn`  
Dashboard: Grafana `markhand-deps`, `markhand-slo`  
Threshold sources: availability SLA 99.5%; latency gates
`G0-SLO-QUERY-P95` (500ms) / `G0-SLO-QUERY-P99` (1000ms) in
`bench/markhand_web/gates.yaml`.

## Prerequisites

- Bounded dependency labels only: `postgres|qdrant|minio|embedding|glm|markhand_ready`.
- Never put connection strings, signed URLs, or credentials in chat/tickets.

## Detection

1. Confirm `probe_success{dependency=...} == 0` and/or elevated embedding/retrieval errors.
2. Check Markhand `/live` (process) vs `/ready` (deps + reconcile fence).
3. Identify which dependency label failed; avoid scraping raw target URLs into alerts.
4. For latency burns, confirm recording series `markhand:retrieval:p95_5m` / `p99_5m`.

## Contain

1. Keep API liveness if possible; fail closed on readiness (do not bypass reconcile fence).
2. Pause workers that hard-depend on the failed system (embed/index/delete as applicable).
3. Enable documented degraded mode only when authz-safe FTS/text fallback is configured
   (see ADRs; do not invent unsafe fallbacks).

## Recover

1. Restore the failed dependency with its own runbook/provider procedure.
2. Verify dependency health endpoints independently (no secrets in command output).
3. Resume Markhand readiness path; wait for reconcile gate when required.
4. Restart paused workers gradually; watch error ratios and queue age.

## Verify

1. `probe_success` returns 1 for the affected dependency.
2. `/ready` 200; retrieval/embedding error ratios under 0.05.
3. Latency recording series under SLO thresholds (0.5s p95 / 1.0s p99).
4. Availability success ratio ≥ 0.995.

## Rollback

- Re-pause workers if probes flap.
- Revert any emergency config that weakened authz or readiness checks.
- If degraded mode was enabled, disable it only after vector path is verified.

## Synthetic evidence

Fixtures: `MarkhandDependencyProbeDown.json`, latency/availability fixtures under
`deploy/observability/fixtures/alerts/`.  
Tabletop: `tt-dependency` — synthetic probe failure only; not a live outage.
