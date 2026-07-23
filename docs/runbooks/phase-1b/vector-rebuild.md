# Vector rebuild

## Detect

- Signature mismatch, empty Qdrant generation, or intentional model cutover.
- Often follows `MarkhandDependencyDown` / Qdrant loss or embedding outage.

```promql
up{job="markhand-qdrant"}
sum(rate(markhand_embedding_batch_duration_seconds_count{outcome=~"failed|error"}[5m]))
```

## Contain

- Keep FTS available; do not mix generations in current search.
- Fence writes if rebuild is destructive; readiness stays false until reconcile.

## Recover

```bash
docker compose -f deploy/compose.poc.yml ps qdrant worker-embedding worker-index
# Enqueue staged backfill / index jobs for current versions (ops tooling)
# Flip generation to active only after verification — no embedding payload dumps
```

1. Ensure active index signature is pinned.
2. Rebuild from PostgreSQL chunks (ADR 0012) when Qdrant snapshot is unusable.
3. Verify with golden retrieval queries only (expected IDs/snippets allowlist).

## Verify

- Retrieval golden queries pass; no shadow/retired generation leakage.
- Drift counters flat; readiness 200 after reconcile-before-ready.
