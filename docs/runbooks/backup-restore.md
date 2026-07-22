# Runbook: Backup and restore (P1B-O03)

Issue: P1B-O03  
Related: ADR 0012, `deploy/backup/**`, migration `0022_expand_runtime_readiness.sql`  
Out of scope: multi-region DR; claiming Profile-B RPO/RTO without a live drill.

## Prerequisites

- POC env from `deploy/.env` (never commit). Backup-specific vars in `.env.example`.
- Narrow credentials only: `MARKHAND_BACKUP_DATABASE_URL`, MinIO backup keys,
  `MARKHAND_BACKUP_SIGNING_KEY` / `MARKHAND_BACKUP_PG_ENCRYPTION_KEY` (hex key ids).
- Tools: `pg_basebackup`, `psql`, `mc`, `curl`, `openssl`, `jq` (or hermetic fakes).
- Docker/compose required for **live** fence/restore; absent → hermetic/static only.
- Targets (gate-valid only on Profile B): RPO ≤ 15m, query-ready RTO ≤ 60m,
  full-vector RTO ≤ 240m.

## Procedure

### Backup

```bash
# From repo root — sets fence, backs up PG→MinIO→Qdrant, writes signed manifest
export MARKHAND_BACKUP_MODE=live   # or hermetic for fixtures
deploy/backup/scripts/backup.sh /var/backups/markhand/$(date -u +%Y%m%dT%H%M%SZ)
# Resume after interrupt: re-run the same command (checkpoint in .state/stage)
```

Manifest path: `<backup-root>/recovery-manifest.json` (HMAC + artifact sha256).

### Restore (default dry-run)

```bash
deploy/backup/scripts/restore.sh /var/backups/markhand/<id>
# Validates manifest/org/schema/signature/migration/checksums; readiness stays false
```

### Restore (destructive apply)

```bash
export MARKHAND_RESTORE_CONFIRM=I_UNDERSTAND_DESTRUCTIVE_RESTORE
export MARKHAND_RESTORE_PGDATA=/var/lib/postgresql/restore
deploy/backup/scripts/restore.sh /var/backups/markhand/<id> --apply
```

Order enforced by scripts:

1. Fence writes (`fence-writes.sh`)
2. Open readiness fence — `markhand_runtime_readiness_open('startup_reconciliation', …)`
3. PostgreSQL PITR to manifest WAL LSN
4. MinIO version inventory / mirror restore
5. Qdrant snapshot **or** `rebuild-vectors-from-pg.sh`
6. Reconcile detect→repair (`reconcile-before-ready.sh`)
7. `markhand_runtime_readiness_try_ready` only after convergence

### PG-only vector rebuild

```bash
deploy/backup/scripts/rebuild-vectors-from-pg.sh 1          # dry-run plan
MARKHAND_RESTORE_CONFIRM=I_UNDERSTAND_DESTRUCTIVE_RESTORE \
  deploy/backup/scripts/rebuild-vectors-from-pg.sh 0
deploy/backup/scripts/reconcile-before-ready.sh repair 0
```

## Verify

1. `deploy/backup/scripts/validate-manifest.sh <manifest> <backup-root>` exits 0.
2. `SELECT ready, generation, detail FROM runtime_readiness WHERE key='startup_reconciliation';`
   is false until reconcile certifies; then true only after convergence.
3. `curl -fsS http://127.0.0.1:${MARKHAND_API_PORT:-8788}/api/v1/health/ready`
4. Missing/orphan: reconcile report under `$MARKHAND_RESTORE_REPORT_DIR`.
5. Do **not** record RPO/RTO pass unless Profile-B live timings were measured.

## Rollback

- Keep prior backup root immutable; do not delete until new restore verified.
- If restore apply fails mid-stage: re-run `restore.sh` (resume via `.state/stage`).
- If readiness incorrectly true: re-open fence with
  `SELECT markhand_runtime_readiness_open('startup_reconciliation', 'manual rollback');`
  and stop API/workers.
- Application rollback never requires DB downgrade (forward migrations only).

## Synthetic / hermetic evidence

```bash
python3 scripts/check-backup-o03.py --self-test
```

Uses `deploy/backup/fixtures/fake-bin/*`. Does not claim live restore or G0-DR pass.
