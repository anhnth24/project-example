# Runbook: Migration safety (P1B-O03)

Issue: P1B-O03
Related: `crates/server/migrations/**`, `scripts/check-migration-manifest.py`,
`deploy/backup/migration/validate-migration-safety.py`
Out of scope: rewriting merged migrations; multi-region DR; DB downgrade rollbacks.

## Prerequisites

- Python 3.12+
- Working tree with `crates/server/migrations/manifest.json` present
- CI/`make check-migrations` green before merge

## Procedure

### Immutable checksums

Merged `.sql` files are content-addressed in `manifest.json`. Any byte change fails:

```bash
python3 scripts/check-migration-manifest.py --check
```

To add a **new** migration only:

1. Create `NNNN_(expand|backfill|cutover|contract|index)_stem.sql`
2. Regenerate manifest checksums:
   `python3 scripts/check-migration-manifest.py --write-manifest`
3. Never edit older files to “fix” — add expand/cutover/contract instead.

### Expand → cutover → contract discipline

For a given feature `stem`, phases must appear in non-decreasing order:

`expand` → optional `backfill`/`index` → `cutover` → `contract`

Rules:

- `cutover` and `contract` require a prior `expand` for the same stem
- `contract` requires a prior `cutover`
- Application rollback must not require DB downgrade (keep expand until contract)

Validate:

```bash
python3 deploy/backup/migration/validate-migration-safety.py --check
python3 deploy/backup/migration/validate-migration-safety.py --self-test
```

### Restore / upgrade compatibility

Backup manifests record `migrationVersion`. Restore validation fails closed on mismatch
with `MARKHAND_BACKUP_MIGRATION_VERSION`. After PG restore, apply **forward-only**
migrations via the server’s normal migrator — never restore a newer schema onto an
older binary without an explicit upgrade plan.

## Verify

1. `make check-migrations` passes (checksums + safety).
2. Empty DB upgrade path covered by server schema tests (`schema_migrations`).
3. No merged migration checksum drift in `git diff crates/server/migrations`.

## Rollback

- Revert the **commit** that added a bad migration before merge.
- After merge: ship a new expand/cutover/contract chain; do not rewrite history files.
- Runtime: keep previous app binary until forward migrations certify readiness.

## Synthetic evidence

Hermetic cases live in `validate-migration-safety.py --self-test` and
`scripts/check-backup-o03.py --self-test` (upgrade compatibility / corrupt manifest).
