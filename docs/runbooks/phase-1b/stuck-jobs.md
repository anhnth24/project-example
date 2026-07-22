# Stuck / dead-letter jobs

## Detect
- Alert `MarkhandQueueGrowth` or rising `markhand_job_queue_age_seconds`.
- `jobs.status in ('leased','dead_letter')` with expired leases.

## Contain
- Pause new uploads if convert/index queues are saturated.
- Do not delete job rows; inspect `last_error` (already truncated, no content).

## Recover
1. Confirm workers healthy (`fileconv-worker --check-config`).
2. Reclaim expired leases (automatic on worker poll).
3. For poison payloads, mark dead-letter and open a scoped repair job.
4. Replay outbox only after DB commit is confirmed.

## Verify
- Queue depth returns below threshold.
- Idempotent re-run does not create duplicate visible versions.
