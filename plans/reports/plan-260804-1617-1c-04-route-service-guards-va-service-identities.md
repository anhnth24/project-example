<!-- generated-done-issue-plan: 1C-04 -->
# 1C-04 — Route/service guards và service identities

Date: 2026-08-04
Source issue: [#105](https://github.com/anhnth24/project-example/issues/105)
Catalog: [`backlog/phase-1c/issues/README.md`](../markhand-web/backlog/phase-1c/issues/README.md)
Phase plan: [`phase-1c-multi-org-security.md`](../markhand-web/phase-1c-multi-org-security.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Route/service guards và service identities**.

## Context

- Phase: `1C`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> CI exact-SHA evidence on `6833f57d94949c75ea36609e1055a1139e097c8a`
> (run [30678318560](https://github.com/anhnth24/project-example/actions/runs/30678318560)):
> changes/static
> [91310040882](https://github.com/anhnth24/project-example/actions/runs/30678318560/job/91310040882),
> `rust`
> [91310110938](https://github.com/anhnth24/project-example/actions/runs/30678318560/job/91310110938),
> `rust-integration`
> [91310110925](https://github.com/anhnth24/project-example/actions/runs/30678318560/job/91310110925)
> (not path-filter skip / soft pass). Task 7–9 reviews Approved. Integration log
> executed `tests/direct_service_authz.rs` (**6 passed; 0 failed**) covering
> `doc.delete` / `doc.publish` / `member.manage` / `audit.view` / `jobs.system`
> direct-service denials; `tests/members.rs` (**13 passed; 0 failed**); and
> `fileconv_worker` unit suite (**9 passed; 0 failed**) including all
> `worker_permissions_tests::*`. `rust` job ran
> `auth::guard_inventory` completeness/invariants green (60-row OpenAPI/route
> inventory) plus worker config fail-closed tests. Dual-layer route+service
> authorize and least-privilege worker identities landed in PR 3.

## Implementation plan

Deny-by-default `authorize`; apply route+service+worker/reconcile;
least-privilege identities.

## Files/modules

The source catalog records implementation and file scope together; see **Implementation plan** above.

## Dependencies / blocks

1C-01/03.

## Acceptance criteria

Allow/deny mỗi permission cả hai layer;
missing-guard inventory, direct-service và worker misuse tests.

## Required tests / evidence

Allow/deny mỗi permission cả hai layer;
missing-guard inventory, direct-service và worker misuse tests.

## Security and migration notes

Không `internal=true` bypass.

## Out of scope

generic ABAC.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- `6833f57d94949c75ea36609e1055a1139e097c8a`

- GitHub sync-closed timestamp: `2026-08-01T04:06:24Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
