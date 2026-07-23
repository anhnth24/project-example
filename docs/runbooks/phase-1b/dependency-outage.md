# PG / Qdrant / MinIO / embedding / provider outage

## Detect

- Alerts: `MarkhandDependencyDown`, `MarkhandEmbeddingErrors`, `MarkhandProviderErrors`
- Queries:

```promql
up{job="markhand-api"}
probe_success{job=~"markhand-(postgres|qdrant|minio|embedding)"}
sum(rate(markhand_embedding_batch_duration_seconds_count{outcome=~"failed|error"}[5m]))
  / clamp_min(sum(rate(markhand_embedding_batch_duration_seconds_count[5m])), 1e-9)
```

```bash
curl -sS -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8788/api/v1/health/ready
docker compose -f deploy/compose.poc.yml --env-file deploy/.env ps postgres qdrant minio mock-embedding api
```

## Contain

- Keep `/api/v1/health/live` up; readiness may stay false.
- Stop mutation workers if PostgreSQL is unavailable:

```bash
docker compose -f deploy/compose.poc.yml --env-file deploy/.env stop \
  worker-convert worker-embedding worker-index
```

- Redact any log capture:

```bash
docker compose -f deploy/compose.poc.yml --env-file deploy/.env logs --tail=100 api \
  2>&1 | python3 deploy/scripts/redact_secrets.py
```

## Recover

```bash
# Example: restore Compose postgres (service name from compose file)
docker compose -f deploy/compose.poc.yml --env-file deploy/.env start postgres
docker compose -f deploy/compose.poc.yml --env-file deploy/.env ps postgres
# App-role DB check (inside postgres container; no password on argv)
docker compose -f deploy/compose.poc.yml --env-file deploy/.env exec -T postgres \
  psql -U markhand_app -d markhand -c 'select current_user;'
```

1. Restore the failed dependency; wait healthy.
2. Qdrant-only loss → rebuild from PG chunks (ADR 0012) via [vector-rebuild](vector-rebuild.md).
3. Embedding/provider errors → restore mock/vLLM/GLM; leave extractive ask online.

## Verify

- `probe_success==1` / `up==1` as applicable; embedding/provider ratios below thresholds.
- `/api/v1/health/live` 200; `/api/v1/health/ready` back to pre-incident baseline
  (may remain non-200 if unrelated readiness probes fail — record baseline explicitly).
- Resume workers after dependencies are healthy.
