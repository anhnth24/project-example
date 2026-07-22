# Runbook: Backup and restore (P1B-O03)

Issue: P1B-O03
Related: ADR 0012, `deploy/backup/**`, migrations `0022`/`0024`
Out of scope: multi-region DR; claiming Profile-B RPO/RTO without a live drill.

## Prerequisites

- Narrow credentials via discrete `MARKHAND_BACKUP_PG*` / MinIO / signing key env
  (or `PGPASSFILE`); **never** put DB URLs/passwords on argv.
- Live apply requires HTTPS/TLS verify-full for Qdrant/MinIO endpoints unless
  `MARKHAND_BACKUP_MODE=hermetic`.
- PostgreSQL method: **`pg_basebackup_streamed_wal`** with PG18
  `backup_label` + `backup_manifest` WAL-Ranges (no LSN fallbacks). Restore
  configures shadow recovery (`restore_command`, `recovery.signal`,
  `recovery_target_lsn`) and verifies before cutover. Envelope:
  `aes-256-ctr-hmac-sha256-v1` (stdlib HKDF/HMAC + libcrypto AES-CTR streaming).
  Continuous PITR stays **blocked** unless archived WAL through the target LSN
  is packaged/checksummed and restore consumes it.
  `deploy/backup/compose.wal-archive.yml` is preparatory only (archive_mode).
- Tools: real `tar`, `pg_basebackup`, `curl` (private configs), `mc` listing via
  `MC_CONFIG_DIR` only; object bodies via signed HTTP (no secret/object-key argv).

## Procedure

### Backup

```bash
deploy/backup/scripts/backup.sh /var/backups/markhand/<id>
```

Produces signed `recovery-manifest.json` binding PG start/stop LSN + timeline,
MinIO encrypted inventory digest/count, Qdrant collection=`markhand_chunks_<sig>`.

### Restore dry-run (default, read-only)

```bash
deploy/backup/scripts/restore.sh /var/backups/markhand/<id>
# No stop/start, no readiness SQL, no store mutation, no checkpoint writes.
```

### Restore apply (destructive)

```bash
export MARKHAND_RESTORE_CONFIRM=I_UNDERSTAND_DESTRUCTIVE_RESTORE
export MARKHAND_RESTORE_TARGET_STATE=/var/markhand/restore-state/<id>
deploy/backup/scripts/restore.sh /var/backups/markhand/<id> --apply
```

Order: fence (real services `api`,`worker-convert`,`worker-index`,`worker-embedding`)
→ open `runtime_readiness` (fail closed) → shadow PGDATA → MinIO shadow prefix
(oldest→newest, new version IDs) → Qdrant shadow collection upload
`priority=snapshot` → bulk reconcile + zero-drift `try_ready`.

Host-path PGDATA extract is refused when Compose uses named volumes; cutover is
via shadow artifacts under `MARKHAND_RESTORE_TARGET_STATE`.

### Reconcile / vector rebuild

```bash
# Live (Docker):
MARKHAND_WORKER_KIND=reconcile \
MARKHAND_RECONCILE_MODE=repair \
MARKHAND_RECONCILE_BULK_ENQUEUE=1 \
MARKHAND_RECONCILE_ONCE=1 \
fileconv-worker
```

Readiness certifies only after verified zero-drift + no pending jobs (0024).

## Verify

1. `validate-manifest.sh` exits 0.
2. `runtime_readiness.ready` false until zero-drift; true only after convergence.
3. MinIO mapping file shows `retainsSourceVersionIds=false`.
4. Do not record RPO/RTO pass without Profile-B live timings.

## Rollback

- Keep backup root immutable; target state holds rollback/shadow artifacts.
- Anti-replay refuses the same manifestSha256 cutover unless explicitly allowed.
- Re-open readiness fence on failed apply.

## Synthetic / hermetic evidence

```bash
python3 scripts/check-backup-o03.py --self-test
```

No live restore or G0-DR claim.
