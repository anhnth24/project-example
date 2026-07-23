# Disk exhaustion

## Detect

- Alert: `MarkhandDiskLow`
- Query:

```promql
node_filesystem_avail_bytes{fstype!~"tmpfs|overlay",mountpoint=~"/|/var|/data|/workspace"}
/
node_filesystem_size_bytes{fstype!~"tmpfs|overlay",mountpoint=~"/|/var|/data|/workspace"}
```

- Host checks (Compose hosts):

```bash
df -h / /var /data /workspace 2>/dev/null || df -h /
docker system df
```

## Contain

- Stop convert/index workers to halt temp growth:

```bash
docker compose -f deploy/compose.poc.yml --env-file deploy/.env stop worker-convert worker-index
```

- Preserve quarantine and trusted object prefixes; do not `rm -rf` MinIO data dirs.

## Recover

1. Expand volume **or** purge only expired temp/workspace directories owned by workers.
2. Never delete trusted objects without reconcile dry-run (see [reconcile-drift](reconcile-drift.md)).
3. Resume workers with resource limits intact:

```bash
docker compose -f deploy/compose.poc.yml --env-file deploy/.env start worker-convert worker-index
docker compose -f deploy/compose.poc.yml --env-file deploy/.env ps worker-convert worker-index
```

## Verify

- Free ratio ≥ 10% on watched mountpoints; `MarkhandDiskLow` inactive.
- Convert/index succeed on a synthetic sample; no unbounded temp growth.
