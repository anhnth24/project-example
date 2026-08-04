<!-- generated-done-issue-plan: 1C-01 -->
# 1C-01 — Organization lifecycle và validated context

Date: 2026-08-04
Base commit: UNKNOWN — not recorded in the source catalog
Source issue: [#102](https://github.com/anhnth24/project-example/issues/102)
Catalog: [`backlog/phase-1c/issues/README.md`](../markhand-web/backlog/phase-1c/issues/README.md)
Phase plan: [`phase-1c-multi-org-security.md`](../markhand-web/phase-1c-multi-org-security.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Organization lifecycle và validated context**.

## Context

- Phase: `1C`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> (run [30629207747](https://github.com/anhnth24/project-example/actions/runs/30629207747)):
> `rust` [91151657403](https://github.com/anhnth24/project-example/actions/runs/30629207747/job/91151657403),
> `web` [91151657388](https://github.com/anhnth24/project-example/actions/runs/30629207747/job/91151657388),
> `rust-integration` [91151657399](https://github.com/anhnth24/project-example/actions/runs/30629207747/job/91151657399)
> (not path-filter skip / soft pass). Integration log executed `orgs` binary tests
> including `create_org_succeeds_and_caller_becomes_owner` and
> `list_orgs_shows_only_the_callers_own_orgs`; job summary reports no ignored tests
> for that binary. Validated-context + lifecycle already landed: membership
> re-verify, fail-closed OrgContext, RLS, `GET /orgs` / `GET /orgs/{id}` /
> `POST /orgs/switch` / `POST /orgs` with owner provision + audit.

## Implementation plan

Org create/list/detail/switch, service/repo/middleware; issue new
context/session after verified membership.

## Files/modules

The source catalog records implementation and file scope together; see **Implementation plan** above.

## Dependencies / blocks

Phase 1B auth/schema.

## Acceptance criteria

Chỉ thấy org của mình;
forged/stale header deny; two-org resolver/integration tests — green on CI
`a62850422dd070e7e1195bfe1d4f1dee0d73566d` run
[30629207747](https://github.com/anhnth24/project-example/actions/runs/30629207747)
(`rust`/`web`/`rust-integration`).

## Required tests / evidence

Chỉ thấy org của mình;
forged/stale header deny; two-org resolver/integration tests — green on CI
`a62850422dd070e7e1195bfe1d4f1dee0d73566d` run
[30629207747](https://github.com/anhnth24/project-example/actions/runs/30629207747)
(`rust`/`web`/`rust-integration`).

## Security and migration notes

Không global org state; audit switch.

## Out of scope

billing/OIDC.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Completion/evidence commits

- `30629207747`
- `91151657388`
- `91151657399`
- `91151657403`
- `a62850422dd070e7e1195bfe1d4f1dee0d73566d`

- GitHub sync-closed timestamp: `2026-08-01T00:18:29Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
