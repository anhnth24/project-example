# Job backlog and dead-letter drain

Use this when queue depth grows, throughput falls below the ingest gate, or jobs
start recording `failed`/`dead_letter` outcomes. Do not add tenant, user, document
or job IDs to metrics while investigating; use logs and authenticated APIs for
specific objects.

## Detection

- `MarkhandJobQueueBacklogWarning`
- `MarkhandJobQueueBacklogOverRecoveryTarget`
- `MarkhandJobThroughputBelowPeakGate`
- `MarkhandJobsInFlightSaturatedWithBacklog`
- `MarkhandJobDeadLetterOrFailedGrowth`
- Metrics: `markhand_jobs_queue_depth`, `markhand_jobs_in_flight`,
  `markhand_jobs_processed_total{job_type,outcome}`

## Triage

```bash
docker compose -f deploy/compose.poc.yml ps
docker compose -f deploy/compose.poc.yml logs --since=30m worker-convert worker-index worker-delete worker-reconcile
curl -fsS http://127.0.0.1:${MARKHAND_POC_SERVER_PORT:-8787}/api/v1/health/ready
```

Check which worker kind is failing from `job_type` and logs:

- `convert` - parser/sandbox/object download problem.
- `index` - embedding, chunking, PostgreSQL or Qdrant problem.
- `delete` - object/vector cleanup problem.
- `reconcile` - drift repair or dependency problem.

If a caller has a specific job ID, inspect the public job endpoints:

```bash
curl -fsS -H "Authorization: Bearer $TOKEN" \
  http://127.0.0.1:${MARKHAND_POC_SERVER_PORT:-8787}/api/v1/jobs/$JOB_ID
curl -fsS -H "Authorization: Bearer $TOKEN" \
  http://127.0.0.1:${MARKHAND_POC_SERVER_PORT:-8787}/api/v1/jobs/$JOB_ID/events
```

## Contain

1. If dependencies are unhealthy, follow
   [Dependency outage](dependency-outage.md) first.
2. If conversion is failing on malformed files, stop only the converter worker while
   preserving API read paths:

   ```bash
   docker compose -f deploy/compose.poc.yml stop worker-convert
   ```

3. If the backlog is healthy but too slow, add workers only through a deployment
   overlay that gives each replica a unique `MARKHAND_WORKER_ID`. The checked-in
   POC compose file uses fixed worker IDs and should not be blindly scaled.
4. Do not blindly requeue failed or dead-letter jobs. Fix the root cause, then
   resubmit the user-visible action that owns the job.

## Recover

- Restart stopped workers after the cause is fixed:

  ```bash
  docker compose -f deploy/compose.poc.yml up -d worker-convert worker-index worker-delete worker-reconcile
  ```

- For index failures on an existing document, enqueue an idempotent reindex:

  ```bash
  curl -fsS -X POST -H "Authorization: Bearer $TOKEN" \
    http://127.0.0.1:${MARKHAND_POC_SERVER_PORT:-8787}/api/v1/documents/$DOCUMENT_ID:reindex
  ```

- For conversion failures caused by a bad upload, ask the owner to upload a corrected
  file after confirming the parser/sandbox issue is fixed.
- For delete/reconcile failures, run the reconcile worker in repair mode using the
  existing `worker-reconcile` service.

## Verify

- `markhand_jobs_queue_depth` drains below 600 and continues downward.
- `markhand_jobs_processed_total{outcome="success"}` grows for the affected
  `job_type`.
- No new `failed` or `dead_letter` increases over 30 minutes.
- Search/ask routes remain inside the P95/P99 SLO panels after the drain.
