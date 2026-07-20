# Disk pressure and backup age

Use this when PostgreSQL, Qdrant or MinIO storage is low on free space, or backups
are too old to meet the 15-minute RPO.

## Detection

- Pending emitter alerts in `deploy/observability/alerts/storage-pending.rules.yml`:
  - `MarkhandDiskFreeSpaceLowPendingEmitter`
  - `MarkhandBackupAgeRpoAtRiskPendingEmitter`
  - `MarkhandBackupAgeRpoExceededPendingEmitter`
- Future metrics:
  - `markhand_disk_free_bytes{component}`
  - `markhand_disk_capacity_bytes{component}`
  - `markhand_backup_last_success_timestamp_seconds{component}`

O01 does not emit these metrics yet. Until the emitter exists, use host and storage
system checks.

## Triage

```bash
docker compose -f deploy/compose.poc.yml ps
docker system df
docker volume ls | grep markhand
df -h
docker compose -f deploy/compose.poc.yml logs --since=30m postgres qdrant minio
```

Check which component is consuming space:

- PostgreSQL data/WAL growth.
- Qdrant segment/snapshot growth.
- MinIO documents, quarantine or artifacts bucket growth.
- Docker image/build cache unrelated to runtime data.

For backup age, inspect the backup system's last-success metadata for PostgreSQL,
Qdrant and MinIO. The SLA target is RPO <= 15 minutes.

## Contain

1. If free space is critically low, stop ingest and conversion first:

   ```bash
   docker compose -f deploy/compose.poc.yml stop worker-convert worker-index
   ```

2. Keep reads online if PostgreSQL and object/vector stores remain consistent.
3. Do not delete MinIO objects or Qdrant collections manually unless a restore or
   reconcile plan is already approved.
4. If backup age exceeds RPO, avoid destructive maintenance until a fresh backup or
   snapshot is captured.

## Recover

- Free non-runtime Docker cache only after confirming it is not a named data volume:

  ```bash
  docker system prune
  ```

- Expand the affected disk or move the named volume according to the host runbook.
- Restart the affected service after storage is available:

  ```bash
  docker compose -f deploy/compose.poc.yml up -d postgres qdrant minio
  ```

- Run backup jobs and record fresh successful timestamps for PostgreSQL, Qdrant and
  MinIO.
- If any store was restored or compacted, run reconciliation before returning ready:

  ```bash
  docker compose -f deploy/compose.poc.yml run --rm worker-reconcile \
    readiness-fence reconciling "post-disk repair"
  docker compose -f deploy/compose.poc.yml up -d worker-reconcile
  docker compose -f deploy/compose.poc.yml run --rm worker-reconcile readiness-fence ready
  ```

## Verify

- Free space is back above 30% headroom for each component.
- Backup age is below 15 minutes for PostgreSQL, Qdrant and MinIO.
- `/api/v1/health/ready` returns `200`.
- Queue depth drains and no new conversion/index failures appear.
