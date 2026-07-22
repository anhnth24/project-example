# Runbook: Vector index rebuild / drift repair

Issue: P1B-O02
Alert: `MarkhandDriftDetected`
Dashboard: `markhand-ops`
Related: ADR 0006/0011. Full backup/restore ordering is **P1B-O03** (out of scope).

## Prerequisites

- Confirm active `MARKHAND_INDEX_SIGNATURE` in `deploy/.env` / API env.
- Workers: `worker-index`, `worker-embedding` (`MARKHAND_WORKER_KIND=index|embedding`).
- **Gap:** there is **no supported operator CLI** for `reconcile --mode=detect|repair` in this repository yet. Runtime reconcile is performed by server/worker code paths / startup fence — do not invent shell commands.

## Detection

```bash
REPO_ROOT="$(git rev-parse --show-toplevel)"
export REPO_ROOT
export POC_WITH_OBSERVABILITY=1
# shellcheck source=deploy/scripts/poc-compose.sh
source "$REPO_ROOT/deploy/scripts/poc-compose.sh"
poc_compose_init

curl -fsS "http://127.0.0.1:${MARKHAND_API_PORT:-8788}/api/v1/health/ready"
# If reconcile fence not ready, ready fails closed — inspect API logs (IDs only)
"${COMPOSE[@]}" logs --tail=200 api worker-index worker-embedding
curl -fsG http://127.0.0.1:9090/api/v1/query \
  --data-urlencode 'query=markhand:drift:increase_10m'
```

## Contain

```bash
"${COMPOSE[@]}" stop worker-embedding worker-index
# Freeze signature/config changes (do not edit MARKHAND_INDEX_SIGNATURE mid-incident)
```

## Recover

1. Restore Qdrant/Postgres health ([dependency-outage](dependency-outage.md)).
2. Restart API so startup reconciliation can re-run:

```bash
"${COMPOSE[@]}" restart api
deploy/scripts/poc-health.sh
```

3. Resume index/embedding workers gradually:

```bash
"${COMPOSE[@]}" start worker-index
"${COMPOSE[@]}" start worker-embedding
```

4. Signature cutover / full rebuild: follow ADR 0011 process; **escalate** for a planned backfill — no ad-hoc SQL deletes of chunks/vectors.

## Verify

1. `/api/v1/health/ready` 200.
2. `markhand:drift:increase_10m` returns to 0.
3. Search smoke (synthetic/fixtures only).

## Rollback

- Stop index/embedding workers if drift returns or readiness fails.
- Keep previous index generation if dual-generation was configured (otherwise escalate).

## Synthetic evidence

Promtool `drift_detected_fires`. No live rebuild claimed.
