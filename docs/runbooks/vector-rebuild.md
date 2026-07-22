# Runbook: Vector index rebuild / drift repair

Issue: P1B-O02  
Alerts: `MarkhandDriftDetected`, `MarkhandReconcileErrors`  
Dashboard: Grafana `markhand-ops`  
Related ADRs: index signature / model migration (0006/0011).  
Backup/restore ordering is owned by **P1B-O03** — this runbook covers rebuild/reconcile only.

## Prerequisites

- Confirm active `index_signature` (model/chunk/dimension/normalize) before rebuild.
- Rebuild is idempotent from PostgreSQL chunks; do not treat Qdrant as SoR.
- No document text in logs/evidence.

## Detection

1. Confirm `markhand:drift:rate_1m > 1` and/or `markhand:reconcile:error_rate_10m > 0`.
2. Inspect bounded drift labels `kind` (`object|vector|index`) and `state`
   (`orphan|missing|stale`) via metrics only.
3. Check whether a signature change or partial outage preceded the drift.

## Contain

1. Keep readiness fail-closed if reconcile fence requires it.
2. Pause embed/index workers if they amplify inconsistent writes.
3. Freeze signature/config changes until repair completes.

## Recover

1. Run detect-mode reconcile first (`mode=detect`) and review counts (IDs only).
2. If safe, run repair-mode reconcile (`mode=repair`) in bounded batches.
3. For signature cutover: enqueue idempotent embedding backfill; wait for active
   generation to become consistent (ADR 0011).
4. Do not delete PostgreSQL chunks to “fix” Qdrant.

## Verify

1. Drift rate returns to ~0; reconcile errors stop.
2. `/ready` 200 with reconcile gate satisfied.
3. Retrieval smoke on synthetic/fixtures corpus (no customer content).
4. Dashboard panels for drift/reconcile stay quiet for ≥15 minutes.

## Rollback

- Stop repair mode if error rate rises; return to detect-only.
- Keep previous index generation readable if dual-generation is configured.
- Re-pause embed workers and escalate if signature mismatch persists.

## Synthetic evidence

Fixtures: `MarkhandDriftDetected.json`, `MarkhandReconcileErrors.json`  
Tabletop: `tt-rebuild` — synthetic drift signal only.
