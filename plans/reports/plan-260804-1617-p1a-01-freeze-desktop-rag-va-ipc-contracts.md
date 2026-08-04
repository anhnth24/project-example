<!-- generated-done-issue-plan: P1A-01 -->
# P1A-01 — Freeze desktop RAG và IPC contracts

Date: 2026-08-04
Base commit: UNKNOWN — not recorded in the source catalog
Source issue: [#68](https://github.com/anhnth24/project-example/issues/68)
Catalog: [`backlog/phase-1a/issues/README.md`](../markhand-web/backlog/phase-1a/issues/README.md)
Phase plan: [`phase-1a-knowledge-extraction.md`](../markhand-web/phase-1a-knowledge-extraction.md)
Status: Done

## Objective

Baseline parity trước khi move code.

## Context

- Phase: `1A`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> Catalog records status as Done.

## Implementation plan

Inventory tests; fixtures top-k/score/snippet/anchor/answer/fallback/stats/
incremental; canonical JSON cho 4 hybrid commands; offline + mock-provider flows.

## Files/modules

`app/src-tauri/src/{knowledge,vector_index}.rs`,
`app/src/lib/{types,ipc}.ts`, backend/frontend contract fixtures.

## Dependencies / blocks

Không.

## Acceptance criteria

CamelCase/answer modes/warnings/tolerance được khóa; undesirable
current behavior cũng được ghi rõ.

## Required tests / evidence

Desktop/core/frontend tests; fixture generation deterministic.

## Security and migration notes

Synthetic content/path, không credential.

## Out of scope

Sửa ranking/concurrency.

## Delivery evidence

### Implementation PRs

- [PR #184](https://github.com/anhnth24/project-example/pull/184) — test: freeze desktop RAG IPC contracts; merged `2026-07-17T16:36:18Z`

### Completion/evidence commits

- `3a2179912c4f362b88bc5e286db1615b38c03ed2`

- GitHub sync-closed timestamp: `2026-07-18T17:06:06Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
