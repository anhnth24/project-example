# Backup and restore

Operational backup/restore **drill ownership is P1B-O03**. This O02 runbook is an
alert-response placeholder only — it does **not** claim a live always-present
backup age series, and it must **not** run capture/restore scripts that set
`ops_fences` or mutate stores.

## Detect

- Alert: `MarkhandBackupStale`
- Query (when O01/O03 exporters emit the series):

```promql
max(markhand_backup_age_seconds) by (store)
```

- O02 does **not** assert an improved/always-present success-only exporter. Treat
  missing or stale backup age as a signal to hand off to the O03 procedure.

## Contain

- Do not paste DB passwords, MinIO keys, or backup encryption secrets into tickets.
- Do **not** run `deploy/backup/backup.sh` from this O02 path — that script sets a
  durable `ops_fences.restore` row and pauses mutating traffic.
- Fence / restore cutover remains O03-owned (`ops_fences`, green namespace guards).

## Recover

### P1B-O03 placeholder

```text
[P1B-O03 PLACEHOLDER] backup capture + restore drill not claimed by O02
Do not invoke deploy/backup/backup.sh or deploy/backup/restore.sh from O02.
Follow the O03 backup/restore procedure with an explicit backup directory,
green targets, durable reconcile.complete, and MARKHAND_RESTORE_CUTOVER=1 only
when O03 evidence requires cutover.
```

## Verify

- Full RPO/RTO / green cutover verification is tracked under **P1B-O03**, not O02.
- After O03 capture, confirm the O03-owned freshness signal and that
  `MarkhandBackupStale` is inactive when that signal is present and fresh.
