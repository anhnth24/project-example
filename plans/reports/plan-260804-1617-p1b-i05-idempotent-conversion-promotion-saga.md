<!-- generated-done-issue-plan: P1B-I05 -->
# P1B-I05 — Idempotent conversion promotion saga

Date: 2026-08-04
Base commit: UNKNOWN — not recorded in the source catalog
Source issue: [#88](https://github.com/anhnth24/project-example/issues/88)
Catalog: [`backlog/phase-1b/issues/README.md`](../markhand-web/backlog/phase-1b/issues/README.md)
Phase plan: [`phase-1b-single-org-poc.md`](../markhand-web/phase-1b-single-org-poc.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Idempotent conversion promotion saga**.

## Context

- Phase: `1B`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> Catalog records status as Done.

## Implementation plan

Checkpoint download/convert/stage/promote/DB/cleanup; immutable version;
publish/current pointer riêng với draft/latest upload; index outbox;
compensation/refund.

## Files/modules

`workers/convert.rs`, `services/{conversion,promotion,artifacts}.rs`,
`db/document_versions.rs`.

## Dependencies / blocks

I01–I04/F06/G1A.

## Acceptance criteria

Retry tạo một visible version/job; trusted chỉ sau success;
fault injection mọi cross-store step; immutable checks.

## Required tests / evidence

Retry tạo một visible version/job; trusted chỉ sau success;
fault injection mọi cross-store step; immutable checks.

## Security and migration notes

Never overwrite original; ACL inherited.

## Out of scope

user merge.

## Delivery evidence

### Implementation PRs

- [PR #244](https://github.com/anhnth24/project-example/pull/244) — feat(server): P1B-I05 idempotent conversion promotion saga; merged `2026-07-20T04:29:10Z`

### Completion/evidence commits

- `35ac9c4018ac91733a12b59d9d43886ed4464e7c`

- GitHub sync-closed timestamp: `2026-07-19T15:17:35Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
