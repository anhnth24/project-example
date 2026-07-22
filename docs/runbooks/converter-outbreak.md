# Runbook: Converter / parser outbreak

Issue: P1B-O02
Alert: `MarkhandConversionErrorOutbreak`
Dashboard: Grafana `markhand-ops`
Threshold: O02-OPS-ERROR-OUTBREAK-RATIO (5%) — operational policy.

## Prerequisites

- `deploy/compose.poc.yml` convert worker (`MARKHAND_WORKER_KIND=convert`)
- Convert network is **internal** (no egress) — see isolation notes in `deploy/README.md`

## Detection

```bash
REPO_ROOT="$(git rev-parse --show-toplevel)"
export REPO_ROOT
export POC_WITH_OBSERVABILITY=1
# shellcheck source=deploy/scripts/poc-compose.sh
source "$REPO_ROOT/deploy/scripts/poc-compose.sh"
poc_compose_init

"${COMPOSE[@]}" logs --tail=300 worker-convert
curl -fsG http://127.0.0.1:9090/api/v1/query \
  --data-urlencode 'query=markhand:conversion:error_ratio_10m'
# Fires when result=~error|other ratio > 0.05 for 10m
```

Break down by bounded `format` label in Grafana (variable `format`) — never log document text.

## Contain

```bash
"${COMPOSE[@]}" stop worker-convert
# Stop client uploads / edge traffic if errors continue to enqueue
```

## Recover

1. If outbreak followed an image/config change, roll back the worker image tag/digest in `deploy/.env` (`MARKHAND_WORKER_IMAGE`) and recreate:

```bash
"${COMPOSE[@]}" up -d --no-deps worker-convert
```

2. Sandbox preflight:

```bash
docker exec "$("${COMPOSE[@]}" ps -q worker-convert)" \
  /usr/local/bin/fileconv-worker --sandbox-preflight
```

3. **Unsupported:** bulk admin requeue of failed convert jobs — not implemented. Escalate remaining dead-letters; new uploads can retry naturally after fix.

## Verify

1. `markhand:conversion:error_ratio_10m` < 0.05.
2. Convert worker healthy; `poc-health.sh` green.
3. Optional format smoke via existing POC evidence scripts (metadata only).

## Rollback

- Stop `worker-convert` again if ratio rebounds.
- Restore previous worker image digest from `deploy/poc/images.lock.json` / prior `.env`.

## Synthetic evidence

Promtool + tabletop `tt-converter-outbreak`. No live outbreak claimed.
