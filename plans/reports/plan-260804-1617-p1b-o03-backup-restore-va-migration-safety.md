<!-- generated-done-issue-plan: P1B-O03 -->
# P1B-O03 — Backup/restore và migration safety

Date: 2026-08-04
Source issue: [#99](https://github.com/anhnth24/project-example/issues/99)
Catalog: [`backlog/phase-1b/issues/README.md`](../markhand-web/backlog/phase-1b/issues/README.md)
Phase plan: [`phase-1b-single-org-poc.md`](../markhand-web/phase-1b-single-org-poc.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Backup/restore và migration safety**.

## Context

- Phase: `1B`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> live blue/green drill 2026-07-26 at `f4f33cd`, run inside
> the O05 soak so it restored a loaded stack rather than an idle one:
> `o03-restore.json` `status=pass`, 0 gaps. Measured attested consistency RPO
> 328s (≤ 900s), query-ready RTO 1099s (≤ 3600s) and full-vector RTO 1099s
> (≤ 14400s) — an order of magnitude above the idle-stack drill (26s/34s)
> because ~180 documents of objects are restored one at a time. The restored
> green API answered a grounded query from the
> restored stores while blue stayed fenced, promote/cutover stayed disabled
> (exit 3), the encrypted destination policy was exercised, and cleanup was
> verified before the report. Report sha256 prefix `66b5045a80925f90`.
> Promote itself remains out of scope until the API consumes durable routing
> plus an independent reconcile target-state attestation.

## Implementation plan

PG PITR, MinIO version inventory, Qdrant snapshot, consistency fence/
manifest, restore order, reconcile-before-ready, vector rebuild.

## Files/modules

`deploy/backup/**`, `deploy/scripts/o03-bluegreen-restore-drill.sh`,
`deploy/scripts/o03-report-from-raw.py`,
`docs/runbooks/phase-1b/backup-restore-o03.md`.

## Dependencies / blocks

F02/F03/F06/I07 + G0-ARCH/G0-SLO.

## Acceptance criteria

Clean restore đạt RPO/RTO; missing/orphan detect; readiness
false until reconcile; PG rebuild; corrupt manifest/upgrade tests.

## Required tests / evidence

Clean restore đạt RPO/RTO; missing/orphan detect; readiness
false until reconcile; PG rebuild; corrupt manifest/upgrade tests.

## Security and migration notes

Encrypted narrow credentials; expand/cutover/contract.

## Out of scope

multi-region DR.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- `66b5045a80925f90`
- `f4f33cd`

- GitHub sync-closed timestamp: `2026-07-26T14:40:17Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
