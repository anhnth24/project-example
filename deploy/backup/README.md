# P1B-O03 — Backup, restore, and migration safety

Production-oriented backup/restore control plane for Markhand Web single-org POC.
Aligns with [ADR 0012](../../docs/adr/0012-backup-recovery-order.md).

**Status: In Progress.** Hermetic/static evidence only until a Profile-B live restore
measures RPO/RTO. **No multi-region DR.**

## Validate (reproducible, no Docker required)

```bash
python3 scripts/check-backup-o03.py
python3 scripts/check-backup-o03.py --self-test
python3 deploy/backup/migration/validate-migration-safety.py --check
make check-backup
```

Evidence is regenerated at `evidence/validation-report.json` (do not hand-edit).
Human summary: `bench/markhand_web/reports/p1b-o03-backup-restore.md`.

## Architecture

| Store | Authority | Backup | Restore |
|---|---|---|---|
| PostgreSQL | **Authority** (visibility, auth, chunks, jobs, readiness) | Encrypted `pg_basebackup` + WAL LSN boundary | First |
| MinIO | Durable originals/artifacts | Version inventory digest (+ optional mirror) | Second |
| Qdrant | Rebuildable | Snapshot + generation + index signature | Third, or PG rebuild |

Cross-store binding: versioned, canonical, HMAC-signed `recovery-manifest.json`
(`schema/recovery-manifest.schema.json`). Manifest stores digests/IDs only —
never secrets, object content, or object key lists.

### Consistency fence

`scripts/fence-writes.sh` prefers a **strict write fence** (stop API/workers via
`deploy/scripts/poc-compose.sh`). When Docker is unavailable it records
`ordered-bounded` mode with an explicit bounded-inconsistency note — never silent.

### Restore order (default dry-run)

1. Validate manifest (structure, HMAC, org/schema/signature/migration, checksums)
2. Fence writes + open `runtime_readiness` (`markhand_runtime_readiness_open`)
3. Restore PostgreSQL → MinIO → Qdrant (or `rebuild-vectors-from-pg.sh`)
4. Reconcile detect/repair (worker path / hermetic fixture)
5. `markhand_runtime_readiness_try_ready` only after verified convergence

Destructive apply requires:

```bash
export MARKHAND_RESTORE_CONFIRM=I_UNDERSTAND_DESTRUCTIVE_RESTORE
deploy/backup/scripts/restore.sh /path/to/backup --apply
```

## Least-privilege env inputs

Copy keys from `deploy/.env.example` backup section. Scripts fail closed when
required vars are missing. Never log `MARKHAND_BACKUP_*KEY*`, passwords, or URLs
with embedded credentials (`lib/common.sh` redaction helpers).

| Variable | Purpose |
|---|---|
| `MARKHAND_BACKUP_DATABASE_URL` | Narrow backup/restore DB role URL |
| `MARKHAND_BACKUP_PG_ENCRYPTION_KEY` / `_KEY_ID` | Envelope key for base tar (hex) |
| `MARKHAND_BACKUP_MINIO_*` | Narrow inventory/mirror credentials |
| `MARKHAND_BACKUP_QDRANT_URL` / `_COLLECTION` | Snapshot API |
| `MARKHAND_BACKUP_SIGNING_KEY` / `_KEY_ID` | Manifest HMAC-SHA256 (32-byte hex) |
| `MARKHAND_INDEX_SIGNATURE` | Active embedding signature binding |
| `MARKHAND_WORKER_ORG_ID` | Org binding in manifest |
| `MARKHAND_BACKUP_MIGRATION_VERSION` | Upgrade compatibility marker |
| `MARKHAND_BACKUP_BIN_DIR` | Optional PATH prefix (hermetic fake CLIs) |
| `MARKHAND_BACKUP_MODE` | `live` \| `hermetic` \| `dry-run` |

## Layout

```text
deploy/backup/
  README.md
  images.lock.json
  schema/recovery-manifest.schema.json
  lib/{common.sh,manifest.py}
  scripts/{backup,restore,fence,reconcile,rebuild}*.sh
  migration/validate-migration-safety.py
  fixtures/fake-bin/*          # hermetic CLIs for CI
  evidence/validation-report.json
```

## Migration safety

`migration/validate-migration-safety.py` enforces:

1. Immutable checksums via `scripts/check-migration-manifest.py` (no edits to merged SQL)
2. Per-stem phase discipline: expand → (backfill\|index)* → cutover → contract

Does not rewrite historical migrations.

## Runbooks

- [`docs/runbooks/backup-restore.md`](../../docs/runbooks/backup-restore.md)
- [`docs/runbooks/migration-safety.md`](../../docs/runbooks/migration-safety.md)
