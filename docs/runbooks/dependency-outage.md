# Runbook: Dependency outage (PG / Qdrant / MinIO / embedding / API ready)

Issue: P1B-O02
Alerts: `MarkhandDependencyProbeDown`, `MarkhandScrapeTargetDown`,
`MarkhandDependencyProbeAbsent`, `MarkhandEmbeddingErrorOutbreak`,
`MarkhandRetrievalErrorOutbreak`, `MarkhandQueryLatencyP95Burn`,
`MarkhandSearchAvailabilityBurn`
Dashboard: `markhand-deps`, `markhand-slo`

## Prerequisites

- Stack: `deploy/compose.poc.yml` (services: `postgres`, `qdrant`, `minio`, `mock-embedding`|`embedding-cpu`, `api`)
- Health script: `deploy/scripts/poc-health.sh` (authoritative host checks)
- Observability overlay (optional): `deploy/observability/compose.observability.yml`
- Probes (in-network):
  - TCP `postgres:5432`
  - HTTP `qdrant:6333/healthz`
  - HTTP `minio:9000/minio/health/live`
  - HTTP `mock-embedding:8080/health` (aiteamvn: `embedding-cpu`)
  - HTTP `api:8787/api/v1/health/ready`

## Detection

```bash
source deploy/scripts/poc-compose.sh && poc_compose_init
deploy/scripts/poc-health.sh
# expected lines: healthy: postgres / qdrant / minio / mock-embedding|embedding-cpu / api-live / api-ready

curl -fsS "http://127.0.0.1:${MARKHAND_API_PORT:-8788}/api/v1/health/live"   # process up
curl -fsS "http://127.0.0.1:${MARKHAND_API_PORT:-8788}/api/v1/health/ready"  # deps + reconcile fence
curl -fsS "http://127.0.0.1:${MARKHAND_QDRANT_HTTP_PORT:-6343}/healthz"
curl -fsS "http://127.0.0.1:${MARKHAND_MINIO_API_PORT:-9010}/minio/health/live"
curl -fsS "http://127.0.0.1:${MARKHAND_EMBEDDING_PORT:-8090}/health"
```

Prometheus (if overlay up):

```bash
curl -fsG http://127.0.0.1:9090/api/v1/query \
  --data-urlencode 'query=probe_success'
curl -fsG http://127.0.0.1:9090/api/v1/query \
  --data-urlencode 'query=up'
```

## Contain

1. Keep `/live` up if possible; **do not bypass** readiness/reconcile fail-closed behavior.
2. Pause workers that need the failed dependency:

```bash
"${COMPOSE[@]}" stop worker-embedding worker-index   # if qdrant/embedding down
"${COMPOSE[@]}" stop worker-convert                  # if postgres/minio down
```

3. Degraded vector fallback only if already configured per ADR — do not invent unsafe bypasses.

## Recover

```bash
"${COMPOSE[@]}" ps
"${COMPOSE[@]}" restart postgres   # or qdrant|minio|mock-embedding|api
deploy/scripts/poc-health.sh
"${COMPOSE[@]}" start worker-convert worker-index worker-embedding
```

Never paste `MARKHAND_DATABASE_URL` / MinIO keys into tickets.

## Verify

1. `poc-health.sh` all healthy.
2. `probe_success == 1` for postgres/qdrant/minio/embedding/markhand_ready.
3. Search P95 / search availability alerts clear (`route=search` only).
4. **Not covered:** filtered-query P99 (blocked alert) — do not claim it.

## Rollback

- Re-stop workers if probes flap.
- Revert emergency config that weakened authz/readiness.

## Synthetic evidence

Promtool cases `dependency_probe_down`, `scrape_target_down`, `search_p95_*`. No live outage claimed.
