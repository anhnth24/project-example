# P1B-O03 backup/restore procedure

O02 must not run these scripts. Full procedure for operators and the live drill.

See `deploy/backup/README.md` for manifest contract, signing, encryption policy,
immutable green allowlists, strict drain, and promote-disabled rules.

```bash
# Capture (requires signing key + encryption or explicit unencrypted dest policy)
deploy/backup/backup.sh

# Green restore only (promote/cutover disabled)
MARKHAND_GREEN_DATABASE_URL=... \
MARKHAND_GREEN_MINIO_BUCKET=... \
MARKHAND_GREEN_QDRANT_COLLECTION=... \
MARKHAND_GREEN_ALLOWLIST_JSON='...' \
deploy/backup/restore.sh /path/to/backup

# Live Compose drill (invokes real backup.sh / restore.sh)
deploy/scripts/o03-bluegreen-restore-drill.sh

# Regenerate report from raw evidence
python3 deploy/scripts/o03-report-from-raw.py \
  bench/markhand_web/reports/phase-1b-gate/raw/o03-<stamp> \
  --out-dir bench/markhand_web/reports/phase-1b-gate
```

Promote remains disabled until the API consumes durable routing and an
independent reconcile target-state attestation exists. Scripts never create
`ops_routing` (no DDL).
