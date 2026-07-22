# Reconcile drift

## Detect
- `MarkhandReconcileDrift` or reconcile report non-zero missing/orphan/stale.

## Contain
- Keep readiness false if fence is active.
- Prefer dry-run before repair.

## Recover
1. `MARKHAND_WORKER_KIND=reconcile` with dry-run.
2. Repair scoped orphans; PostgreSQL tombstones win.
3. Re-run repair; confirm idempotence.

## Verify
- Drift counters return to zero; search/citation denial still holds for deleted docs.
