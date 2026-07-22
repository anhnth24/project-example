# Runbook: Stuck / dead-letter jobs

Issue: P1B-O02  
Alerts: `MarkhandQueueOldestAgeHigh`, `MarkhandQueueDepthWarning`, `MarkhandDeadLetterJobs`  
Dashboard: Grafana `markhand-queue`  
Threshold sources: `docs/markhand-web-sla-targets.md` (queue age ≤ 120 min),
`bench/markhand_web/gates.yaml#G0-CAP-INGEST-THROUGHPUT` (depth warning derived).

## Prerequisites

- Read-only access to Prometheus/Grafana and server/worker logs (redacted).
- Ability to pause workers / drain queues in the target environment.
- Do **not** dump job payloads, document text, prompts, or secrets into tickets.

## Detection

1. Confirm alert series:
   - `markhand:queue:oldest_age_seconds_max > 7200`
   - and/or `markhand:queue:depth_max > 600`
   - and/or `markhand:job:dead_letter_rate_5m > 0`
2. Identify bounded `queue` / `job_type` label (`convert|embed|index|reconcile|delete`).
3. Correlate with `markhand_job_transitions_total` rates (enqueue/claim/finish/result).
4. Check `/ready` and dependency probes (see [dependency-outage](dependency-outage.md)).

## Contain

1. Stop admission of new heavy work if depth/age is growing unbounded:
   - scale upload admission / disable non-critical ingest (environment-specific).
2. Pause the affected worker pool only (keep API liveness if possible).
3. Do not delete queue rows blindly; snapshot counts by `job_type`/`result` only.

## Recover

1. Inspect recent worker errors for the bounded `job_type` (no content logging).
2. If dependency fault: follow [dependency-outage](dependency-outage.md) first.
3. If poison jobs: move/mark dead_letter per ops policy; requeue only idempotent jobs.
4. Resume one worker replica; confirm `claim`/`finish` success rates recover.
5. Gradually restore admission.

## Verify

1. `markhand_queue_oldest_age_seconds` trending down for the affected queue.
2. Depth below 600 and dead_letter rate returns to 0.
3. `/ready` returns 200; search smoke succeeds without cross-tenant data.
4. Resolve alert only after `for` windows clear.

## Rollback

- Re-pause workers if finish errors return.
- Re-enable admission only after age/depth improve for ≥10 minutes.
- If requeue amplified failures, leave jobs in dead_letter and escalate.

## Synthetic evidence

Fixture: `deploy/observability/fixtures/alerts/MarkhandQueueOldestAgeHigh.json`  
Tabletop: `deploy/observability/fixtures/tabletop/o02-tabletop.json` (`tt-queue-stuck`)  
These are synthetic evaluations; they do **not** claim a live outage was exercised.
