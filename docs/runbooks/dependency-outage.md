# Dependency outage

Use this when PostgreSQL, Qdrant, MinIO, migrations, index-signature config or the
restore/reconcile fence makes the API unready.

## Detection

- `MarkhandReadinessEndpointFailing`
- `MarkhandReadinessScrapeMissing`
- `MarkhandMetricsEndpointFailing`
- Query SLO burn alerts when dependency latency propagates to `/api/v1/search`,
  `/api/v1/ask` or `/api/v1/ask/stream`.
- `/api/v1/health/ready` returns `503` with code `dependency_unavailable`,
  `configuration_invalid` or `not_reconciled`.

## Triage

```bash
curl -i http://127.0.0.1:${MARKHAND_POC_SERVER_PORT:-8787}/api/v1/health/ready
docker compose -f deploy/compose.poc.yml ps
docker compose -f deploy/compose.poc.yml logs --since=30m server postgres qdrant minio worker-reconcile
curl -fsS http://127.0.0.1:${MARKHAND_POC_QDRANT_HTTP_PORT:-16333}/healthz
curl -fsS http://127.0.0.1:${MARKHAND_POC_MINIO_API_PORT:-19000}/minio/health/live
```

Readiness checks:

- PostgreSQL connection and `markhand_schema_migrations` checksums.
- Qdrant `GET /healthz`.
- MinIO `GET /minio/health/live`.
- `MARKHAND_INDEX_SIGNATURE` digest format when configured.
- O03 readiness fence state. `not_reconciled` means restore or reconciliation is
  intentionally blocking readiness.

## Contain

1. Keep the server failing readiness until the dependency is safe.
2. If restore or repair is in progress, set the fence explicitly:

   ```bash
   docker compose -f deploy/compose.poc.yml run --rm worker-reconcile \
     readiness-fence reconciling "dependency outage repair"
   ```

3. Stop write-heavy workers if the backing dependency is unstable:

   ```bash
   docker compose -f deploy/compose.poc.yml stop worker-convert worker-index worker-delete
   ```

4. If Qdrant is down but PostgreSQL is healthy, keep authz-safe text/FTS fallback
   enabled for query paths that support degraded mode. Do not bypass authorization
   hydration.

## Recover

- Restart or replace the failed dependency:

  ```bash
  docker compose -f deploy/compose.poc.yml up -d postgres qdrant minio minio-init
  ```

- Restart the server after configuration or migration fixes:

  ```bash
  docker compose -f deploy/compose.poc.yml up -d server
  ```

- Run reconciliation after store restore or drift:

  ```bash
  docker compose -f deploy/compose.poc.yml up -d worker-reconcile
  ```

- Clear the readiness fence only after reconciliation and consistency checks pass:

  ```bash
  docker compose -f deploy/compose.poc.yml run --rm worker-reconcile readiness-fence ready
  ```

## Verify

```bash
curl -fsS http://127.0.0.1:${MARKHAND_POC_SERVER_PORT:-8787}/api/v1/health/ready
curl -fsS http://127.0.0.1:${MARKHAND_POC_SERVER_PORT:-8787}/api/v1/metrics
```

- `markhand_http_requests_total{route="/api/v1/health/ready",status=~"2.."}` grows.
- No new readiness 5xx samples for 15 minutes.
- Search/ask routes return authorized results or documented fallback responses.
- Job backlog drains after workers are restarted.
