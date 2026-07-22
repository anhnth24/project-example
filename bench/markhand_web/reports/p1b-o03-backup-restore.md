# P1B-O03 evidence — backup/restore and migration safety

Status: **In Progress**.
`claims_live_restore`: **false**
`claims_rpo_rto_pass`: **false**
Profile-B DR gate: `unresolved` (targetMatch=false)

## Evidence classes

### implemented

- deploy/backup/** scripts and recovery manifest tooling
- docs/runbooks/backup-restore.md
- docs/runbooks/migration-safety.md
- migration expand→cutover→contract validator

### static

- layout/executable scripts
- digest-pinned images.lock.json
- runbook sections + destructive confirmation
- migration safety against crates/server/migrations
- secret hygiene scan under deploy/backup

### hermetic

- success backup + dry-run restore
- corrupt manifest
- wrong org/schema/signature
- missing artifact/snapshot/WAL
- command failure fail-closed
- interrupted/resume
- path traversal/symlink rejection
- destructive confirmation
- readiness fence until reconcile
- upgrade migrationVersion mismatch
- redaction
- PG-only vector rebuild dry-run

### pending_live

- Docker compose clean-host restore
- measured RPO <= 15m / query-ready RTO <= 60m / full-vector RTO <= 240m
- live missing/orphan detection against real MinIO/Qdrant

## Commands

```bash
python3 scripts/check-backup-o03.py --self-test
python3 deploy/backup/migration/validate-migration-safety.py --check
make check-backup
```

Machine report: `deploy/backup/evidence/validation-report.json` (ok=true, hermeticTestsRun=13).

## Non-claims / blockers

- Docker unavailable or unused — no live restore claim.
- Profile-B RPO/RTO gate evidence unresolved.
- Multi-region DR out of scope.
