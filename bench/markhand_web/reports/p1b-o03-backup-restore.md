# P1B-O03 evidence — backup/restore and migration safety (rebuild)

Status: **In Progress**.
`claims_live_restore`: **false**
`claims_rpo_rto_pass`: **false**
PostgreSQL method: `pg_basebackup_streamed_wal` (continuous PITR: `blocked_unless_archive_wal_packaged_and_consumed`).

## Evidence classes

### implemented

- streamed WAL restorable PG backup + EtM envelope metadata
- MinIO version/delete-marker inventory + restore mapping
- Qdrant collection identity from index signature
- dry-run read-only; fence quiescence; target-bound restore state
- zero-drift readiness certification (migration 0024)
- bulk enqueue + reconcile-once worker path
- recovery-manifest.schema.json enforced (unknown fields fail)

### static

- digest pins
- runbooks
- migration safety + SQL semantic policy
- wal-archive overlay labeled preparatory only
- no host cryptography package lock claim

### hermetic

- backup+dry-run+apply
- drift keeps ready false / zero-drift ready
- corrupt/duplicate JSON
- org/schema/signature/migration mismatch
- missing artifact / command failure
- path traversal/symlink/destructive confirm
- OpenSSL CTR+HMAC roundtrip/tamper/wrong-key/truncation
- schema enforcement + schema/code drift mutation
- PITR blocked without packaged archive WAL
- anti-replay target binding

### pending_live

- Docker compose restore with shadow cutover
- continuous PITR only after packaged archive WAL + restore consume
- Profile-B RPO/RTO measurements

## Non-claims / blockers

- No live Docker restore or Profile-B RPO/RTO pass.
- Continuous PITR blocked unless archived WAL through target LSN is packaged/checksummed and consumed on restore; wal-archive overlay is preparatory only.
- Multi-region DR out of scope.
