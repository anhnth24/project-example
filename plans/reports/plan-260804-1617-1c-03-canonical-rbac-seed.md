<!-- generated-done-issue-plan: 1C-03 -->
# 1C-03 — Canonical RBAC seed

Date: 2026-08-04
Source issue: [#104](https://github.com/anhnth24/project-example/issues/104)
Catalog: [`backlog/phase-1c/issues/README.md`](../markhand-web/backlog/phase-1c/issues/README.md)
Phase plan: [`phase-1c-multi-org-security.md`](../markhand-web/phase-1c-multi-org-security.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Canonical RBAC seed**.

## Context

- Phase: `1C`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> CI exact-SHA evidence on `a62850422dd070e7e1195bfe1d4f1dee0d73566d`
> (run [30629207747](https://github.com/anhnth24/project-example/actions/runs/30629207747)):
> `rust` [91151657403](https://github.com/anhnth24/project-example/actions/runs/30629207747/job/91151657403),
> `web` [91151657388](https://github.com/anhnth24/project-example/actions/runs/30629207747/job/91151657388),
> `rust-integration` [91151657399](https://github.com/anhnth24/project-example/actions/runs/30629207747/job/91151657399)
> (not path-filter skip / soft pass). Integration log executed `role_catalog` tests
> including `permissions_table_contains_exactly_active_catalog_keys ... ok` and
> `canonical_matrix_matches_builtin_role_catalog_fixture ... ok`; job summary reports
> no ignored tests for that binary. Canonical fixture
> `crates/server/openapi/builtin-role-catalog.json` is the sole built-in
> active/reserved matrix; OpenAPI references it (no embedded grants); web imports it
> for role order; DB parity tests compare exact active keys/grants. Historical
> migration `0030` catalog + per-org provision unchanged. P1C.2 disposition: matrix
> follows the fixture. Guard inventory for active operations remains 1C-04 / later PRs.

## Implementation plan

Permission constants + DB seed owner/admin/editor/viewer; immutable
system roles; OpenAPI/web fixture consumers.

## Files/modules

The source catalog records implementation and file scope together; see **Implementation plan** above.

## Dependencies / blocks

Phase 1B role schema.

## Acceptance criteria

Matrix đúng/idempotent,
duplicate/missing/immutable mutation tests; UI không hard-code matrix — green on CI
`a62850422dd070e7e1195bfe1d4f1dee0d73566d` run
[30629207747](https://github.com/anhnth24/project-example/actions/runs/30629207747)
(`role_catalog` in `rust-integration`).

## Required tests / evidence

Matrix đúng/idempotent,
duplicate/missing/immutable mutation tests; UI không hard-code matrix — green on CI
`a62850422dd070e7e1195bfe1d4f1dee0d73566d` run
[30629207747](https://github.com/anhnth24/project-example/actions/runs/30629207747)
(`role_catalog` in `rust-integration`).

## Security and migration notes

Stable keys, expand/backfill.

## Out of scope

custom role builder.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- `a62850422dd070e7e1195bfe1d4f1dee0d73566d`

- GitHub sync-closed timestamp: `2026-08-01T00:18:34Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
