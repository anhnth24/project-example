# ADR 0012: Backup and recovery authority/order

- Status: Accepted
- Date: 2026-07-18
- Decision key: `backup-recovery-order`
- Owners: operations-owner, storage-owner, retrieval-owner
- Approver: Phase 0 architecture gate
- Related issues/PRs: P0-10; ADR 0006; ADR 0008; ADR 0009; ADR 0011

## Context

Markhand Web stores authoritative metadata/chunks/jobs in PostgreSQL, originals
and derived artifacts in MinIO, and vectors in Qdrant. A restore runbook must
define which system is authoritative and in what order services become
query-ready. Without that order, a partial Qdrant or object restore could make
deleted/revoked content visible or hide required originals.

## Decision

PostgreSQL is the authority for restore, visibility, authorization, document
state, chunk text, index generation pointers, jobs, audit and reconciliation.
MinIO originals are durable source artifacts and cannot be reconstructed from
PostgreSQL or Qdrant. Qdrant is rebuildable from PostgreSQL chunks plus the
active index signature, but snapshots may be restored first to reduce RTO.

Restore order:

1. **Fence writes and capture manifest:** stop API mutations/workers, record the
   target recovery point, application version, migration version, active index
   generation, MinIO inventory marker and Qdrant snapshot marker.
2. **Restore PostgreSQL first:** recover to the selected point in time and run
   migrations only in the approved forward direction.
3. **Restore MinIO originals/derived artifacts:** restore bucket versioning or
   backup inventory to at least the PostgreSQL recovery point. Missing originals
   block full readiness for affected documents.
4. **Restore or rebuild Qdrant:** prefer a matching snapshot for query-ready RTO,
   then reconcile against PostgreSQL. If no matching snapshot exists, rebuild the
   active generation from PostgreSQL chunks.
5. **Run reconciliation before readiness:** detect missing/orphan/stale objects
   and vectors. PostgreSQL tombstones win over Qdrant/MinIO leftovers.
6. **Open query-ready mode:** allow authorized read/search when PostgreSQL,
   MinIO integrity checks and either restored vectors or documented text/FTS
   fallback meet the query-ready target.
7. **Complete full-vector rebuild:** finish active generation rebuild/verification
   before claiming full-vector RTO.

Targets are:

- RPO <= 15 minutes;
- query-ready RTO <= 60 minutes;
- full-vector RTO <= 240 minutes.

Current P0-10 evidence is an offline deterministic smoke drill using recorded
spike lifecycle evidence with `targetMatch=false`; it does not pass G0-DR gates.
Profile B on-prem-reference restore drills are required before production Phase 0
exit.

## Consequences

- Positive: authorization and delete state are restored before any vector result
  can be served.
- Positive: Qdrant loss is recoverable if PostgreSQL chunks and MinIO originals
  are intact.
- Negative: MinIO loss can be service-degrading or data-losing for originals even
  when derived text remains in PostgreSQL.
- Operational: backups must include a manifest with cross-store markers and
  checksums, not only separate tool snapshots.

## Alternatives considered

- Restore Qdrant first and open search early: rejected because vectors cannot
  authorize themselves and may include deleted/revoked documents.
- Treat MinIO as reconstructable from Markdown/chunks: rejected; originals and
  some derived artifacts are source-of-record objects.
- Claim offline synthetic timings as DR evidence: rejected. Offline smoke only
  validates runbook shape and marker emission.

## Verification

Offline smoke:

```bash
python3 bench/markhand_web/scripts/run_restore_drill.py --self-test
python3 bench/markhand_web/scripts/run_restore_drill.py
```

Inspect:

- `bench/markhand_web/restore/summary.json`
- `bench/markhand_web/reports/restore-drill.md`

Production verification remains:

- component-loss restore on `on-prem-reference`;
- manifest checksums for PostgreSQL, MinIO and Qdrant;
- measured RPO, query-ready RTO and full-vector RTO;
- reconciliation proving no unauthorized/deleted content is query-visible.

## Exception lifecycle

The offline P0-10 smoke exception has narrow scope:

- Owner: operations-owner
- Scope: Phase 0 runbook and synthetic marker validation only
- Expiry: before production Phase 0 exit or any Profile B DR claim
- Required revalidation: live on-prem-reference restore drill with targetMatch=true
