<!-- generated-done-issue-plan: P1B-O02 -->
# P1B-O02 — Dashboards, alerts và runbooks

Date: 2026-08-04
Source issue: [#98](https://github.com/anhnth24/project-example/issues/98)
Catalog: [`backlog/phase-1b/issues/README.md`](../markhand-web/backlog/phase-1b/issues/README.md)
Phase plan: [`phase-1b-single-org-poc.md`](../markhand-web/phase-1b-single-org-poc.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Dashboards, alerts và runbooks**.

## Context

- Phase: `1B`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> live tabletop 2026-07-26 at `f4f33cd`: `o02-alerts.json`
> `status=pass`, 31 passes / 0 fails, no blockers. Real fault executed against
> the POC stack: `MarkhandDependencyDown` fired at 150s while Postgres was
> stopped and went absent 24s after restore, both snapshots taken from the live
> Prometheus `/api/v1/alerts` (no synthetic promtool mirror). Also covered:
> promtool rule + unit tests, dashboard/datasource parameterization, runbook
> DCRV, PG restore arm-before-stop failpoint matrix, live reconcile worker
> dry-run→repair→idempotent plus the `worker-reconcile-oneshot` compose job, and
> a clean provenance + broad secret scan. Report sha256 prefix
> `56f0475a26fd174d`.

## Implementation plan

SLO/queue/disk/dependency alerts; runbooks jobs/parser/outage/rebuild/disk/
GLM/key rotation.

## Files/modules

`deploy/observability/**`, `docs/runbooks/phase-1b/**`,
`deploy/scripts/o02-alert-tabletop.sh`, `deploy/scripts/o02-pg-restore-guard.sh`,
`deploy/scripts/redact_secrets.py`, `deploy/scripts/test_redact_secrets.py`,
`deploy/compose.poc.yml` (`worker-reconcile-oneshot` profile / job),
`crates/server/src/{bin/worker.rs,workers/reconcile.rs,jobs/**,db/jobs.rs}`,
`crates/server/tests/deletion_reconcile.rs` (live reconcile worker drills).

## Dependencies / blocks

F02/F06/I03/O01 + G0-SLO.

## Acceptance criteria

Trigger từng alert; runbook detection→contain→recover→verify;
rule validation/fault/tabletop evidence; compose oneshot dry-run/repair/clean or
documented deployment gap.

## Required tests / evidence

Trigger từng alert; runbook detection→contain→recover→verify;
rule validation/fault/tabletop evidence; compose oneshot dry-run/repair/clean or
documented deployment gap.

## Security and migration notes

No tenant/document high-cardinality labels.

## Out of scope

staffing.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- `56f0475a26fd174d`
- `f4f33cd`

- GitHub sync-closed timestamp: `2026-07-26T14:40:15Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
