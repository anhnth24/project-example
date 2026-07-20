# Vector index rebuild and reindex

Use this when the vector index is missing, stale, restored from snapshot, or rejected
because the configured embedding/index signature changed.

## Detection

- `MarkhandFilteredQueryP99LatencySloBurn` or retrieval panels show high
  `markhand_retrieval_latency_seconds_bucket{stage,outcome}` latency.
- Search/ask returns dependency errors while PostgreSQL remains healthy.
- Readiness returns `not_reconciled` during restore or reconcile.
- Logs mention `index signature change requires an explicit reindex`.
- `markhand_jobs_processed_total{job_type="index",outcome=~"failed|dead_letter"}`
  increases.

## Triage

```bash
curl -i http://127.0.0.1:${MARKHAND_POC_SERVER_PORT:-8787}/api/v1/health/ready
docker compose -f deploy/compose.poc.yml logs --since=30m server worker-index worker-reconcile qdrant
curl -fsS http://127.0.0.1:${MARKHAND_POC_QDRANT_HTTP_PORT:-16333}/healthz
```

Identify the scope:

- Single document/version needs reindex.
- Qdrant collection or snapshot restore needs reconcile.
- Embedding runtime signature changed and old vectors must not be reused.
- Qdrant is unavailable, so query must use authz-safe degraded retrieval where
  supported.

## Contain

1. Set readiness to reconciling during rebuilds that can serve stale vectors:

   ```bash
   docker compose -f deploy/compose.poc.yml run --rm worker-reconcile \
     readiness-fence reconciling "vector rebuild"
   ```

2. Keep authorization checks in PostgreSQL as the source of truth. Never return text
   directly from Qdrant payloads without live hydration.
3. If Qdrant is unavailable, use extractive/text fallback for ask/search paths that
   support it and communicate degraded mode to users.

## Recover

- Reindex one document idempotently:

  ```bash
  curl -fsS -X POST -H "Authorization: Bearer $TOKEN" \
    http://127.0.0.1:${MARKHAND_POC_SERVER_PORT:-8787}/api/v1/documents/$DOCUMENT_ID:reindex
  ```

- For a restored or repaired store, run the reconcile worker in repair mode:

  ```bash
  docker compose -f deploy/compose.poc.yml up -d worker-reconcile
  ```

- For a signature change, rebuild affected documents with the approved embedding
  runtime and keep the old generation inactive until verification passes.
- If Qdrant data was lost, restore a snapshot first when available; otherwise
  reindex from PostgreSQL chunks and trusted artifacts.

## Verify

- `markhand_jobs_processed_total{job_type="index",outcome="success"}` grows and
  failed/dead-letter growth stops.
- Search returns current, authorized versions only.
- Ask citations resolve through `/api/v1/documents/{documentId}/versions/{versionId}/citations:resolve`.
- Retrieval p95/p99 returns under the 500 ms / 1000 ms SLO thresholds.
- Clear the fence only after reconciliation:

  ```bash
  docker compose -f deploy/compose.poc.yml run --rm worker-reconcile readiness-fence ready
  ```
