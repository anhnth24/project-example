<!-- generated-done-issue-plan: P1B-F01 -->
# P1B-F01 — Extend server skeleton với runtime POC

Issue closed: 2026-07-19
Source issue: [#78](https://github.com/anhnth24/project-example/issues/78)
Catalog: [`backlog/phase-1b/issues/README.md`](../markhand-web/backlog/phase-1b/issues/README.md)
Phase plan: [`phase-1b-single-org-poc.md`](../markhand-web/phase-1b-single-org-poc.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Extend server skeleton với runtime POC**.

## Context

- Phase: `1B`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> Catalog records status as Done.

## Implementation plan

Mở rộng `crates/server` API/worker skeleton từ F-02/F-07 với runtime
dependencies, application state, graceful shutdown và các config fields đã được
Phase 0 phê duyệt. Không tạo lại workspace/config conventions.

## Files/modules

`crates/server/{Cargo.toml,src/{lib,main,config,error,state}.rs}`,
`src/bin/worker.rs`.

## Dependencies / blocks

G0-ARCH.

## Acceptance criteria

API/worker compile độc lập; invalid URL/secret/limit/issuer/
signature không start; config/env/shutdown/table tests; secrets không `Debug`.

## Required tests / evidence

API/worker compile độc lập; invalid URL/secret/limit/issuer/
signature không start; config/env/shutdown/table tests; secrets không `Debug`.

## Security and migration notes

Unsafe defaults chỉ dev mode.

## Out of scope

business routes/HA.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- UNKNOWN — no completion/evidence commit is cited in the catalog status.

- GitHub sync-closed timestamp: `2026-07-19T15:17:08Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
