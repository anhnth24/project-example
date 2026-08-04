<!-- generated-done-issue-plan: P1A-06 -->
# P1A-06 — Rank, citation và grounded answer

Issue closed: 2026-07-18
Source issue: [#73](https://github.com/anhnth24/project-example/issues/73)
Catalog: [`backlog/phase-1a/issues/README.md`](../markhand-web/backlog/phase-1a/issues/README.md)
Phase plan: [`phase-1a-knowledge-extraction.md`](../markhand-web/phase-1a-knowledge-extraction.md)
Status: Done

## Objective

Reusable hybrid merge, anchors và grounding.

## Context

- Phase: `1A`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> merged to `master` after parity and prompt-injection gates passed.

## Implementation plan

Cosine/RRF/rerank/sort; snippet/page-slide-sheet anchor; extractive answer;
citation validator; separate LLM calls.

## Files/modules

`crates/knowledge/src/{rank,citation,ask}.rs`, golden tests.

## Dependencies / blocks

DTO/query.

## Acceptance criteria

Top-k/citation/answer parity trong tolerance; invented citation
fallback; server caller không kéo desktop features.

## Required tests / evidence

Tie/NaN/overlap/anchor/snippet/grounding/golden.

## Security and migration notes

Untrusted passages không thành instruction.

## Out of scope

Learned reranker/streaming.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- UNKNOWN — no completion/evidence commit is cited in the catalog status.

- GitHub sync-closed timestamp: `2026-07-18T17:06:18Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
