<!-- generated-done-issue-plan: F-01 -->
# F-01 — Architecture boundaries và dependency rules

Issue closed: 2026-07-17
Source issue: [#46](https://github.com/anhnth24/project-example/issues/46)
Catalog: [`backlog/phase-f/issues/README.md`](../markhand-web/backlog/phase-f/issues/README.md)
Phase plan: [`phase-f-engineering-foundation.md`](../markhand-web/phase-f-engineering-foundation.md)
Status: Done

## Objective

Khóa dependency direction và module responsibilities trước scaffold.

## Context

- Phase: `F`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> merged to `master` via PR #160.

## Implementation plan

Viết architecture boundary ADR; define allowed/forbidden
dependencies; route→service→repository; tenant context rule; browser/Tauri split;
automated `cargo tree`/import checks. Bootstrap minimum CODEOWNERS, issue/PR
template, Definition of Ready/Done và security-review triggers để govern chính
Phase F; F-12 hoàn thiện và kiểm chứng workflow.

## Files/modules

`docs/adr/0001-web-boundaries.md` (new),
`docs/conventions/dependencies.md` (new), `.github/CODEOWNERS`,
`.github/{ISSUE_TEMPLATE,PULL_REQUEST_TEMPLATE}.md`, CI boundary scripts.

## Dependencies / blocks

Không; blocks F-02 và mọi crate/web implementation.

## Acceptance criteria

Core không framework/storage; knowledge pure mặc định;
server không reverse-depend desktop; web không Tauri; vendor không dependency.

## Required tests / evidence

Positive/negative sample boundary checks trong CI;
architecture diagram và approver.

## Security and migration notes

Tenant context bắt buộc ở repository rule; migration N/A.

## Out of scope

Storage trait tổng quát và business implementation.

## Delivery evidence

### Implementation PRs

- [PR #160](https://github.com/anhnth24/project-example/pull/160) — docs: establish Markhand Web architecture boundaries; merged `2026-07-17T08:08:21Z`

### Recorded commit/SHA references

- `ab3cc97f41a2021a8074500f808e220df9d54bbe`

- GitHub sync-closed timestamp: `2026-07-17T11:06:29Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
