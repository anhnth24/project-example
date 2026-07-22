# Runbook: Host root filesystem pressure

Issue: P1B-O02
Alert: `MarkhandHostRootFilesystemLow`
Dashboard: `markhand-ops`
Source: `bench/markhand_web/workload-profile.yaml` `hardware.headroomPercent.disk=30`.

## Scope (honest)

- **Monitored:** host root filesystem via node_exporter (`--path.rootfs=/host` → PromQL `mountpoint="/"`).
- **Unavailable/blocked:** Docker named-volume free-space attribution for Postgres (`pgdata`) or MinIO (`miniodata`). Do not claim volume-specific metrics. See blocked alert `MarkhandNamedVolumeDiskLow`.
- Durable purge of PG/MinIO data requires backup confirmation (**O03**) — out of scope here.

## Prerequisites

- Observability overlay: `deploy/observability/compose.observability.yml` (sets `REPO_ROOT` binds)
- Host tools: `docker`, `df`, `curl`
- Safety: never delete Postgres/MinIO data from this runbook

## Detection

```bash
# Safe array init (no word-splitting). Include observability overlay for node-exporter.
REPO_ROOT="$(git rev-parse --show-toplevel)"
export REPO_ROOT
export POC_WITH_OBSERVABILITY=1
# shellcheck source=deploy/scripts/poc-compose.sh
source "$REPO_ROOT/deploy/scripts/poc-compose.sh"
poc_compose_init

df -h /
docker system df
curl -fsG http://127.0.0.1:9090/api/v1/query \
  --data-urlencode 'query=markhand:disk:host_root_free_ratio'
# Alert when host root free ratio < 0.30 for 10m
```

## Contain

```bash
# Stop writers to slow growth (does not delete data)
"${COMPOSE[@]}" stop worker-convert worker-index worker-embedding
# Block new uploads at the edge / stop client traffic
```

## Recover

Safe host/Docker disk diagnostics and ephemeral reclaim only (no data deletion):

```bash
# Diagnostics (supported; read-only / non-destructive)
docker system df -v
docker builder du
df -h /
df -i /

# Ephemeral reclaim only — build cache / unused images (NOT volumes, NOT containers with data)
docker builder prune -f
docker image prune -f

# Optional: inspect which containers contribute most writable-layer growth (no truncate/delete of mounts)
docker ps -q | while read -r id; do
  docker inspect --format '{{.Name}} {{.SizeRootFs}}' "$id" 2>/dev/null || true
done
```

Expand host capacity / free space via infra. **Do not** delete MinIO objects, Postgres data dirs, or named volumes from this runbook.

Resume writers when `markhand:disk:host_root_free_ratio` ≥ 0.30:

```bash
"${COMPOSE[@]}" start worker-convert worker-index worker-embedding
"$REPO_ROOT/deploy/scripts/poc-health.sh"
```

## Verify

1. `markhand:disk:host_root_free_ratio` ≥ 0.30 (`mountpoint="/"`).
2. Workers healthy; queue age not ENOSPC-driven.
3. Alert resolves.

## Rollback

- Re-stop writers if free ratio falls again.
- Escalate before touching durable stores.

## Synthetic evidence

Promtool `host_root_filesystem_low_fires` / `host_root_filesystem_healthy_non_firing`. No host disk was filled. No live outage claimed.
