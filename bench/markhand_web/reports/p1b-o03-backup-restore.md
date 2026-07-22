# P1B-O03 evidence — backup/restore and migration safety

Status: **In Progress**.
`claims_live_restore`: **false**
`claims_rpo_rto_pass`: **false**
PostgreSQL method: `pg_basebackup_streamed_wal` (continuous PITR: `blocked_unless_archive_wal_packaged_and_consumed`).

## Evidence classes

### implemented

- PG18 backup_label/backup_manifest WAL-Ranges + junk rejection
- shadow recovery configure+verify via pinned pg_ctl/docker path
- campaign identity + atomic checkpoints + cutover receipts from ops
- MinIO encrypted opaque object bodies; keys not used as paths
- Qdrant v1.18.2 schema parse + alias cutover after verify
- streaming EtM crypto; readiness sealed campaign; fence opt-in
- migration base-ref + SQL lexer; JSON NaN reject; appVersion range

### static

- digest pins
- runbooks
- migration safety + base-ref anchor
- wal-archive overlay preparatory only

### contract

- stateful fake CLI/HTTP adapters (no hermetic shortcuts)
- junk WAL / Qdrant schema / cross-manifest resume
- MinIO traversal + encrypted bodies
- TLS/credential non-argv
- apply success + drift/repair + stage failure paths

### pending_live

- Docker compose restore with measured RPO/RTO
- continuous PITR only after packaged archive WAL + restore consume

## Remaining blockers

- No Docker daemon in this environment for live compose cutover drill
- Profile-B RPO≤15m / RTO gates unresolved
- Live MinIO/Qdrant/PG with real TLS certs not exercised here
