# Runbook: Disk exhaustion

Issue: P1B-O02
Alert: `MarkhandDiskLow`
Dashboard: `markhand-ops`
Source: `bench/markhand_web/workload-profile.yaml` `hardware.headroomPercent.disk=30`.

## Prerequisites

- node-exporter from `deploy/observability/compose.observability.yml` (host rootfs + POC volumes)
- Mountpoints monitored: `/`, `/var/lib/postgresql`, `/data` (MinIO)
- Durable purge of PG/MinIO data requires backup confirmation (**O03**) — out of scope here

## Detection

```bash
df -h /
docker system df
source deploy/scripts/poc-compose.sh && poc_compose_init
"${COMPOSE[@]}" exec postgres df -h /var/lib/postgresql || true
curl -fsG http://127.0.0.1:9090/api/v1/query \
  --data-urlencode 'query=markhand:disk:free_ratio'
# Alert when free ratio < 0.30 for 10m
```

## Contain

```bash
"${COMPOSE[@]}" stop worker-convert worker-index worker-embedding
# Block new uploads at the edge / stop client traffic
```

## Recover

Safe ephemeral cleanup only:

```bash
docker builder prune -f
# Rotate/truncate container logs if safe in your environment
"${COMPOSE[@]}" logs --tail=0 api >/dev/null
```

Expand volume / free capacity via infra. **Do not** indiscriminately delete MinIO versions or Postgres data.

Resume writers when free ratio ≥ 0.30:

```bash
"${COMPOSE[@]}" start worker-convert worker-index worker-embedding
deploy/scripts/poc-health.sh
```

## Verify

1. `markhand:disk:free_ratio` ≥ 0.30 on monitored mounts.
2. Workers healthy; queue age not ENOSPC-driven.
3. Alert resolves.

## Rollback

- Re-stop writers if free ratio falls again.
- Escalate before touching durable stores.

## Synthetic evidence

Promtool `disk_low_fires`. No host disk was filled.
