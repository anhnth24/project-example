<!-- generated-done-issue-plan: P2-01 -->
# P2-01 — React/Vite workspace và UI foundations

Date: 2026-08-04
Source issue: [#116](https://github.com/anhnth24/project-example/issues/116)
Catalog: [`backlog/phase-2/issues/README.md`](../markhand-web/backlog/phase-2/issues/README.md)
Phase plan: [`phase-2-web-spa.md`](../markhand-web/phase-2-web-spa.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **React/Vite workspace và UI foundations**.

## Context

- Phase: `2`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> #311. Web workspace, no Tauri import, CI job `web` xanh.

## Implementation plan

Tạo `web/` scripts/layout; copy browser-safe tokens/icons/primitives.

## Files/modules

The source catalog records implementation and file scope together; see **Implementation plan** above.

## Dependencies / blocks

Không.

## Acceptance criteria

Build/test độc lập; no Tauri import;
typecheck/lint/unit/dependency-boundary; desktop vẫn xanh.

## Required tests / evidence

Build/test độc lập; no Tauri import;
typecheck/lint/unit/dependency-boundary; desktop vẫn xanh.

## Security and migration notes

Dependency/license scan.

## Out of scope

shared package/redesign desktop.

## Delivery evidence

### Implementation PRs

- [PR #311](https://github.com/anhnth24/project-example/pull/311) — Web wave 0 remainder and wave 1: client, SSE, mocks, login shell, scope-safe org switch; merged `2026-07-27T03:09:05Z`

### Recorded commit/SHA references

- `370c8f738af25f8becb4ecde709057b4ed70a8d4`

- GitHub sync-closed timestamp: `2026-07-27T09:48:46Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
