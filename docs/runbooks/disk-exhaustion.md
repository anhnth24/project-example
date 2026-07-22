# Runbook: Disk exhaustion

Issue: P1B-O02  
Alert: `MarkhandDiskLow`  
Dashboard: Grafana `markhand-ops`  
Threshold source: `bench/markhand_web/workload-profile.yaml`
`hardware.headroomPercent.disk = 30` → free ratio must stay ≥ 0.30.

## Prerequisites

- Host/volume access for bounded mountpoints:
  `/`, `/var/lib/postgresql`, `/data`, `/var/lib/markhand`.
- Do not delete MinIO object versions or PG data without backup confirmation
  (full backup/restore is **P1B-O03**).

## Detection

1. Confirm `markhand:disk:free_ratio{mountpoint=...} < 0.30` for ≥10m.
2. Identify which bounded mountpoint is low (dashboard variable `mountpoint`).
3. Correlate with ingest rate, temp conversion dirs, and log volume growth.

## Contain

1. Pause ingest/convert workers writing to the affected volume.
2. Block new large uploads at admission if disk is critically low (<10% free).
3. Snapshot current free/used bytes (numbers only; no object keys/content).

## Recover

1. Clear safe ephemeral paths (worker temp, old rotated logs) only.
2. Expand volume / free capacity via infra procedure.
3. If object-store volume: lifecycle incomplete multipart uploads per MinIO ops guide
   (no indiscriminate version purge).
4. Resume writers gradually after free ratio ≥ 0.30.

## Verify

1. `markhand:disk:free_ratio` ≥ 0.30 on all monitored mountpoints.
2. Ingest/convert success rates recover; queue age not climbing solely due to ENOSPC.
3. `/ready` healthy; alert resolves.

## Rollback

- Re-pause writers if free ratio falls again.
- Restore any mistakenly deleted ephemeral config from known-good package.
- Escalate before touching PostgreSQL/MinIO durable data.

## Synthetic evidence

Fixture: `MarkhandDiskLow.json`  
Tabletop: `tt-disk` — synthetic free-ratio values only; no host disk was filled.
