<!-- generated-done-issue-plan: P1A-09 -->
# P1A-09 — Thin Tauri adapters

Date: 2026-08-04
Base commit: UNKNOWN — not recorded in the source catalog
Source issue: [#76](https://github.com/anhnth24/project-example/issues/76)
Catalog: [`backlog/phase-1a/issues/README.md`](../markhand-web/backlog/phase-1a/issues/README.md)
Phase plan: [`phase-1a-knowledge-extraction.md`](../markhand-web/phase-1a-knowledge-extraction.md)
Status: Done

## Objective

Desktop commands delegate shared crate, IPC giữ nguyên.

## Context

- Phase: `1A`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> Catalog records status as Done.

## Implementation plan

Tauri giữ state/settings/path load/spawn_blocking/error mapping; delegate
rebuild/stats/search/ask; retain legacy commands; remove duplicate only sau parity.

## Files/modules

`app/src-tauri/src/{knowledge,vector_index,intelligence,lib}.rs`,
Cargo manifests.

## Dependencies / blocks

Pure logic + stores.

## Acceptance criteria

Command/payload/result unchanged; source adapter mỏng; legacy index
behavior documented; no duplicate algorithm.

## Required tests / evidence

Backend/frontend contract + manual rebuild/search/offline/LLM fallback.

## Security and migration notes

Path jail và secret-safe errors giữ ở desktop.

## Out of scope

UI/IPC rename/async redesign.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Completion/evidence commits

- UNKNOWN — no completion/evidence commit is cited in the catalog status.

- GitHub sync-closed timestamp: `2026-07-18T17:06:24Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
