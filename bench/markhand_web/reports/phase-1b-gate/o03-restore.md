# P1B-O03 backup/restore drill

- Status: `in_progress`
- Capture window: `2`s
- Restore-green seconds: `7`s
- consistencyRpoPass: `None`
- queryReadyRtoPass: `None`
- Baseline ready: `200`
- Post-restore live: `200`
- Post-restore ready: `503`
- Cleanup verified: `True`
- Raw: `/workspace/bench/markhand_web/reports/phase-1b-gate/raw/o03-20260723T122307Z`

## Passes

- baseline ready 200
- consistency backup refused when app write gate unavailable
- hermetic auth/schema/symlink/traversal/malformed/pgpass/mc guards
- proc canary: no MinIO secret on mc argv
- concurrent backup refused under session advisory lock
- manifest mode 0600 (umask 077)
- backup.sh capture (captureWindow 2s)
- minio normalized history written
- normalized MinIO history (type/size/hash; no versionId/ts)
- restore.sh refuses without green targets
- restore refuses missing MinIO allowlist
- restore.sh refuses wrong green allowlist
- restore.sh refuses blue bucket alias
- restore.sh refuses blue collection alias
- existing allowlisted MinIO target refused before mutation
- restore.sh refuses tampered artifacts
- restore-green OK; promote disabled
- cutover/promote disabled (exit 3)
- no query-ready claim (ready HTTP 503 after restore query)
- post-restore API live 200
- blue restore fence retained (no false cutover)
- no raw dumps in evidence
- verified cleanup before report
- reproducible raw→report

## Exact gaps

- app mutation write-gate not integrated (consistency backup refused unless REQUIRE=0)
- promote/cutover disabled: API does not consume durable routing + independent reconcile target-state attestation
- encrypted backup destination not exercised (POC explicit_poc_tmp_only policy)

## Notes

- Report generated solely from raw evidence (reproducible).
- Promote/cutover disabled until API consumes routing + durable attestation.
- consistencyRpoPass/queryReadyRtoPass are null (not claimed).
- Status remains in_progress until all Sol acceptance items close.
- No query-ready claim: post-restore ready HTTP=503 (not 200).
