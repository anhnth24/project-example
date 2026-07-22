# P1B-O03 — Backup, restore, and migration safety (rebuild)

Fail-closed control plane aligned with ADR 0012.

**Status: In Progress.** Hermetic/static evidence only. Continuous PITR stays
**blocked** unless archived WAL through the target LSN is packaged, checksummed,
and consumed on restore. `compose.wal-archive.yml` is preparatory only. No
live/RPO/RTO claim.

## Validate

```bash
python3 scripts/check-backup-o03.py --self-test
make check-backup
python3 deploy/backup/migration/validate-migration-safety.py --check
```

## What is implementation-ready

| Area | Behavior |
|---|---|
| PostgreSQL | `pg_basebackup -Ft -X stream` → encrypted `base.tar` + `pg_wal.tar`; start/stop LSN + timeline; verified with real `tar` + OpenSSL CTR round-trip |
| Encryption | `aes-256-ctr-hmac-sha256-v1`: HKDF-SHA256 + HMAC-SHA256 (stdlib) + host OpenSSL AES-256-CTR (encrypt-then-MAC; not GCM/AEAD); salt/iv/mac/AAD/digests bound in signed manifest |
| Schema | `recovery-manifest.schema.json` enforced by `schema_validate.py` (unknown fields fail); duplicate keys rejected by `strictjson` |
| MinIO | Version + delete-marker inventory (encrypted); restore oldest→newest; **new** version IDs; mapping artifact |
| Qdrant | Collection `markhand_chunks_<indexSignature>`; upload `priority=snapshot` |
| Fence | Real services only; quiescence verified; `ordered-bounded` ⇒ `writesFenced=false` |
| Dry-run | Strictly read-only |
| Readiness | Migration `0024` — drift/error cannot `try_ready`; bulk enqueue + `MARKHAND_RECONCILE_ONCE` |

## Not claimed

- Live Docker restore / Profile-B RPO≤15m / RTO gates
- Continuous archive PITR merely because `compose.wal-archive.yml` exists
- Multi-region DR
