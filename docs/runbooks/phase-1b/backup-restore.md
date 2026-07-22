# Backup and restore

Follow ADR 0012 and `deploy/backup/{backup,restore}.sh`.

## Detect
- `MarkhandBackupStale` or failed backup job.

## Contain
- Fence writes before restore.
- Refuse restore when manifest checksum mismatches.

## Recover
1. Restore PostgreSQL first.
2. Restore MinIO inventory/objects.
3. Restore or rebuild Qdrant.
4. Run reconcile; keep readiness false until clean.

## Verify
- RPO ≤ 15m and query-ready RTO ≤ 60m on the reference profile.
- Missing/orphan detection runs; unauthorized content stays suppressed.
