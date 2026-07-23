# Stuck / dead-letter jobs and quota pressure

## Detect

- Alerts: `MarkhandQueueGrowth`, `MarkhandQueueAgeHigh`, `MarkhandQuotaExceeded`
- Queries:

```promql
max(markhand_job_queue_depth) by (job_type)
max(markhand_job_queue_age_seconds) by (job_type)
increase(markhand_quota_reservation_total{outcome="exceeded"}[15m])
```

## Contain

- Pause new uploads if convert/index queues are saturated.
- Do not delete job rows; inspect truncated `last_error` only (no content).
- For quota storms, stop bulk ingest clients; do not raise limits blindly.

## Recover

```bash
# Worker health (Compose POC)
docker compose -f deploy/compose.poc.yml ps worker-convert worker-embedding worker-index
docker compose -f deploy/compose.poc.yml --env-file deploy/.env logs --tail=100 worker-convert \
  2>&1 | python3 deploy/scripts/redact_secrets.py

# Optional: reclaim is automatic on worker poll; confirm process alive
docker compose -f deploy/compose.poc.yml exec -T worker-convert /usr/local/bin/fileconv-worker --check-config
```

1. Restart unhealthy workers after dependency readiness is green.
2. For poison payloads, leave dead-letter and open a scoped repair job.
3. Replay outbox only after DB commit is confirmed.
4. Quota `exceeded` is expected under intentional limits — fix client backlog or plan capacity; never log reservation tokens.

## Verify

- Queue depth `< 100` and age `< 600s`; alerts inactive.
- Idempotent re-run does not create duplicate visible versions.
- `MarkhandQuotaExceeded` clears after sustained `increase(...[15m]) ≤ 10`.
