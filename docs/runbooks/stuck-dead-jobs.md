# Runbook: Stuck / dead-letter jobs

Issue: P1B-O02
Alerts: `MarkhandQueueOldestAgeHigh`, `MarkhandQueueDepthWarning`, `MarkhandDeadLetterJobs`
Dashboard: Grafana `markhand-queue`
Sources: queue age ≤ 120 min (`docs/markhand-web-sla-targets.md`); depth warn 600 derived from `G0-CAP-INGEST-THROUGHPUT`; dead-letter is event policy `O02-OPS-DEAD-LETTER-EVENT` (not error-ratio).

## Prerequisites

- POC stack: `deploy/compose.poc.yml` via `deploy/scripts/poc-compose.sh`
- Host tools: `docker`, `curl`
- Env file: `deploy/.env` (from `deploy/.env.example`); project `MARKHAND_COMPOSE_PROJECT=markhand-poc`
- Do **not** log job payloads, document text, prompts, or secrets

## Detection

1. Confirm alert in Prometheus/Alertmanager (or Grafana `markhand-queue`).
2. On the host:

```bash
source deploy/scripts/poc-compose.sh && poc_compose_init
"${COMPOSE[@]}" ps
deploy/scripts/poc-health.sh
curl -fsS "http://127.0.0.1:${MARKHAND_API_PORT:-8788}/api/v1/health/ready"
# expected: HTTP 200 JSON readiness (or non-200 if deps/reconcile fence failing)
```

3. Inspect worker kinds (compose services `worker-convert`, `worker-index`, `worker-embedding`):

```bash
"${COMPOSE[@]}" logs --tail=200 worker-convert worker-index worker-embedding
# Look for job.dead_lettered / lease / sandbox errors — IDs only, no content
```

4. Metrics (if observability overlay is up on :9090):

```bash
curl -fsG http://127.0.0.1:9090/api/v1/query \
  --data-urlencode 'query=markhand:queue:oldest_age_seconds_max'
curl -fsG http://127.0.0.1:9090/api/v1/query \
  --data-urlencode 'query=markhand:job:dead_letter_increase_5m'
```

## Contain

1. Stop admission of new uploads if age/depth is climbing (disable client traffic / edge ingress).
2. Scale down the affected worker only:

```bash
"${COMPOSE[@]}" stop worker-convert   # or worker-index / worker-embedding
```

3. **Do not** DELETE job rows from PostgreSQL. There is **no supported admin requeue CLI** in this repo yet — treat dead-lettered jobs as escalate/contain until a job-admin tool ships (future gap).

## Recover

1. If dependency/health failed first, follow [dependency-outage](dependency-outage.md).
2. For convert sandbox failures:

```bash
convert_id="$("${COMPOSE[@]}" ps -q worker-convert)"
docker exec "$convert_id" /usr/local/bin/fileconv-worker --sandbox-preflight
# expected: exit 0
```

3. Restart one worker:

```bash
"${COMPOSE[@]}" start worker-convert
"${COMPOSE[@]}" logs -f --tail=100 worker-convert
```

4. Local non-compose worker (dev only), from `deploy/dev/worker.env.example`:

```bash
export MARKHAND_WORKER_KIND=convert   # or index|embedding
cargo run --release -p fileconv-server --bin fileconv-worker
```

## Verify

1. `deploy/scripts/poc-health.sh` passes.
2. Queue age trending down; `markhand:job:dead_letter_increase_5m` returns to 0.
3. `/api/v1/health/ready` stays 200 for ≥10 minutes.
4. Resolve alert after the PromQL window clears.

## Rollback

- `"${COMPOSE[@]}" stop <worker>` if errors return.
- Keep admission closed until age < 7200s and depth < 600.
- Escalate: open incident with job_type + error codes only (no payloads).

## Synthetic evidence

`promtool test rules deploy/observability/prometheus/tests/alerts_test.yml`
(`dead_letter_single_event_fires_and_resolves`). No live outage claimed.
