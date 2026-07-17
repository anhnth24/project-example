# Server migrations

Phase 1B adds schema files here. Until then, this folder intentionally has no business
tables or database client dependency.

## Contract

- File name: `NNNN_<expand|backfill|cutover|contract>_<subject>.sql`.
- Add a migration header describing owner, phase, lock/data risk and compatibility.
- Run `python3 scripts/check-migration-manifest.py --write-manifest` when adding a
  reviewed migration; commit the updated `manifest.json`.
- CI runs `--check`; existing checksums cannot change.

See [`docs/conventions/sql-migrations.md`](../../../docs/conventions/sql-migrations.md).
