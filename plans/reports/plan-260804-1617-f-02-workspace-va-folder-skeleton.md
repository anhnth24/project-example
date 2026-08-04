<!-- generated-done-issue-plan: F-02 -->
# F-02 — Workspace và folder skeleton

Date: 2026-08-04
Base commit: UNKNOWN — not recorded in the source catalog
Source issue: [#47](https://github.com/anhnth24/project-example/issues/47)
Catalog: [`backlog/phase-f/issues/README.md`](../markhand-web/backlog/phase-f/issues/README.md)
Phase plan: [`phase-f-engineering-foundation.md`](../markhand-web/phase-f-engineering-foundation.md)
Status: Done

## Objective

Tạo khung compile được cho knowledge/server/web/deploy/docs/bench.

## Context

- Phase: `F`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> Catalog records status as Done.

## Implementation plan

Add workspace members với minimal libraries/binaries; module
READMEs/ownership; Vite web shell; deploy/dev placeholders; không copy business
logic. Chốt root pnpm workspace/lockfile policy cho `app/` + `web/`; pin Node,
pnpm, task runner và Compose requirements; thêm bootstrap/version-check command.

## Files/modules

`Cargo.toml`, `crates/{knowledge,server}/`, `web/`, `deploy/dev/`,
`docs/{adr,conventions,runbooks}/`, `bench/markhand_web/`.

## Dependencies / blocks

F-01; blocks coding/tooling/dev environment issues.

## Acceptance criteria

Cargo workspace và web build; server API/worker binaries start
help/config validation only; no cyclic/forbidden deps; JS workspace/lockfile policy
và host tool versions được máy kiểm tra.

## Required tests / evidence

`cargo metadata/check`, bootstrap/version check,
`pnpm install --frozen-lockfile`, app+web build, tree/import boundary.

## Security and migration notes

No credential/default public bind; no DB migration.

## Out of scope

Auth/schema/routes/jobs.

## Delivery evidence

### Implementation PRs

- [PR #160](https://github.com/anhnth24/project-example/pull/160) — docs: establish Markhand Web architecture boundaries; merged `2026-07-17T08:08:21Z`

### Completion/evidence commits

- `ab3cc97f41a2021a8074500f808e220df9d54bbe`

- GitHub sync-closed timestamp: `2026-07-17T11:06:31Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
