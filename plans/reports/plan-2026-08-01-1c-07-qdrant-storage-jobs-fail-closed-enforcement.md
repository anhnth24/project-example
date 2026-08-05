<!-- generated-done-issue-plan: 1C-07 -->
# 1C-07 — Qdrant/storage/jobs fail-closed enforcement

Issue closed: 2026-08-01
Source issue: [#109](https://github.com/anhnth24/project-example/issues/109)
Catalog: [`backlog/phase-1c/issues/README.md`](../markhand-web/backlog/phase-1c/issues/README.md)
Phase plan: [`phase-1c-multi-org-security.md`](../markhand-web/phase-1c-multi-org-security.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Qdrant/storage/jobs fail-closed enforcement**.

## Context

- Phase: `1C`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> CI exact-SHA evidence on `6833f57d94949c75ea36609e1055a1139e097c8a`
> (run [30678318560](https://github.com/anhnth24/project-example/actions/runs/30678318560)):
> `rust`
> [91310110938](https://github.com/anhnth24/project-example/actions/runs/30678318560/job/91310110938),
> `rust-integration`
> [91310110925](https://github.com/anhnth24/project-example/actions/runs/30678318560/job/91310110925)
> (not path-filter skip / soft pass). Integration log executed
> `tests/storage.rs` (**ok**; Qdrant/MinIO fail-closed + cross-org overwrite/
> dimension/missing-scope/object-key denials including
> `qdrant_connection_failure_fails_closed_as_transport`,
> `qdrant_unresponsive_endpoint_times_out_as_transport`,
> `cross_org_point_overwrite_rejected`,
> `same_org_different_collection_cannot_overwrite`,
> `missing_scope_rejects_without_network_side_effects`). `rust`/`rust-integration`
> also ran fast forged-payload / malformed / empty-scope unit pins under
> `storage::qdrant::tests::*` and retrieval forged-candidate deny. Signed-URL N/A
> (capability tokens). Connected cross-org denial suite remains **1C-12**.

## Implementation plan

Mandatory org+non-empty collection filter; PG payload validation;
authorize preview/download/export/job/SSE; abort in-flight on ACL change.

## Files/modules

The source catalog records implementation and file scope together; see **Implementation plan** above.

## Dependencies / blocks

1C-05/06.

## Acceptance criteria

Missing/malformed/timeout/mismatch deny;
Qdrant failure, forged payload, job ID, stream revoke, signed URL replay tests.

## Required tests / evidence

Missing/malformed/timeout/mismatch deny;
Qdrant failure, forged payload, job ID, stream revoke, signed URL replay tests.

## Security and migration notes

No signed URL logs.

## Out of scope

public sharing/CDN.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- `6833f57d94949c75ea36609e1055a1139e097c8a`

- GitHub sync-closed timestamp: `2026-08-01T04:06:30Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
