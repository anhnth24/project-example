# API latency burn

## Detect

- Alert: `MarkhandApiLatencyBurn`
- Query:

```promql
histogram_quantile(0.95, sum(rate(markhand_http_request_duration_seconds_bucket[5m])) by (le, route))
```

- Dashboard panel: **API request latency p95**

## Contain

- Freeze deploys / reindex bursts.
- Shed non-critical load (bulk reindex, soak).
- Confirm embedding and Qdrant legs are not both timing out:

```promql
histogram_quantile(0.95, sum(rate(markhand_retrieval_leg_duration_seconds_bucket[5m])) by (le, leg, outcome))
```

## Recover

```bash
# Readiness / live (no auth secrets in output)
curl -sS -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8788/api/v1/health/live
curl -sS -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8788/api/v1/health/ready

# Inspect API + worker health (Compose POC example)
docker compose -f deploy/compose.poc.yml ps api worker-convert worker-embedding worker-index
docker compose -f deploy/compose.poc.yml --env-file deploy/.env logs --tail=100 api \
  2>&1 | python3 deploy/scripts/redact_secrets.py
```

1. Scale read path / workers within capacity ADR limits.
2. Keep one-leg degradation enabled for retrieval.
3. Do not dump request bodies, prompts, or tokens into tickets.

## Verify

- p95 `< 2` for ≥10m; alert inactive.
- Traces/logs contain no secrets or document content.
