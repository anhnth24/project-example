<!-- generated-done-issue-plan: P1A-08 -->
# P1A-08 — Persistent HNSW desktop feature

Date: 2026-08-04
Base commit: UNKNOWN — not recorded in the source catalog
Source issue: [#75](https://github.com/anhnth24/project-example/issues/75)
Catalog: [`backlog/phase-1a/issues/README.md`](../markhand-web/backlog/phase-1a/issues/README.md)
Phase plan: [`phase-1a-knowledge-extraction.md`](../markhand-web/phase-1a-knowledge-extraction.md)
Status: Done

## Objective

Move optional ANN cache, SQLite vẫn authority.

## Context

- Phase: `1A`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> Catalog records status as Done.

## Implementation plan

Manifest/partition/rebuild/search/clear; legacy signature compatibility;
corrupt/mismatch fallback exact cosine.

## Files/modules

`crates/knowledge/src/desktop/hnsw.rs`, legacy HNSW fixture,
source `vector_index.rs`.

## Dependencies / blocks

Feature scaffold + vectors/identity.

## Acceptance criteria

Round-trip parity; corruption không mất data; location/threshold
không đổi; `hnsw_rs` optional.

## Required tests / evidence

Corrupt/count/signature/atomic replacement/feature matrix.

## Security and migration notes

Validate manifest bounds/path.

## Out of scope

HNSW tuning/Qdrant.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Completion/evidence commits

- UNKNOWN — no completion/evidence commit is cited in the catalog status.

- GitHub sync-closed timestamp: `2026-07-18T17:06:22Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
