<!-- generated-done-issue-plan: P1A-05 -->
# P1A-05 — Query, local vectors và embedding plan

Date: 2026-08-04
Source issue: [#72](https://github.com/anhnth24/project-example/issues/72)
Catalog: [`backlog/phase-1a/issues/README.md`](../markhand-web/backlog/phase-1a/issues/README.md)
Phase plan: [`phase-1a-knowledge-extraction.md`](../markhand-web/phase-1a-knowledge-extraction.md)
Status: Done

## Objective

Tách pure query/embedding preparation.

## Context

- Phase: `1A`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> merged to `master` after local parity and security gates passed.

## Implementation plan

Normalization, feature hash/vector norm, provider plan, dimension check,
FTS escape; HTTP client vẫn ở core; giữ local fallback semantics.

## Files/modules

`crates/knowledge/src/{query,embedding}.rs`, tests; source desktop module.

## Dependencies / blocks

Shared types.

## Acceptance criteria

Output parity; query rỗng/punctuation safe; mismatch/fallback không
đổi; không Tauri/settings/filesystem.

## Required tests / evidence

Vietnamese/punctuation/determinism/provider mock/dim mismatch.

## Security and migration notes

Credential-bearing URL không vào signature/error.

## Out of scope

Async client/new tokenizer.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- UNKNOWN — no completion/evidence commit is cited in the catalog status.

- GitHub sync-closed timestamp: `2026-07-18T17:06:15Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
