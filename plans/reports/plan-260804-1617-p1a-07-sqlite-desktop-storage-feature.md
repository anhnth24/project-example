<!-- generated-done-issue-plan: P1A-07 -->
# P1A-07 — SQLite desktop storage feature

Date: 2026-08-04
Source issue: [#74](https://github.com/anhnth24/project-example/issues/74)
Catalog: [`backlog/phase-1a/issues/README.md`](../markhand-web/backlog/phase-1a/issues/README.md)
Phase plan: [`phase-1a-knowledge-extraction.md`](../markhand-web/phase-1a-knowledge-extraction.md)
Status: Done

## Objective

Move SQLite persistence, bỏ reverse dependency vào Tauri.

## Context

- Phase: `1A`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> merged to `master` with atomic storage and legacy DB gates.

## Implementation plan

Schema/metadata/vector/incremental/FTS/hydration; API nhận DB path +
caller-supplied corpus; Tauri giữ path jail/load.

## Files/modules

`crates/knowledge/src/desktop/sqlite.rs`, legacy DB fixture,
`app/src-tauri/src/{knowledge,intelligence}.rs`.

## Dependencies / blocks

Shared APIs stable.

## Acceptance criteria

Legacy DB parity; incremental/scope/signature/fallback giữ nguyên;
không gọi data_root/load_documents/resolve_within; optional rusqlite.

## Required tests / evidence

Empty/legacy/changed/scope/corrupt-dim/persistence.

## Security and migration notes

Caller chịu path jail; schema additive hoặc explicit rebuild.

## Out of scope

PostgreSQL/perf redesign.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- UNKNOWN — no completion/evidence commit is cited in the catalog status.

- GitHub sync-closed timestamp: `2026-07-18T17:06:20Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
