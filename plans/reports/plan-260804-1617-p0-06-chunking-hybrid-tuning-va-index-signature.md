<!-- generated-done-issue-plan: P0-06 -->
# P0-06 — Chunking, hybrid tuning và index signature

Date: 2026-08-04
Source issue: [#63](https://github.com/anhnth24/project-example/issues/63)
Catalog: [`backlog/phase-0/issues/README.md`](../markhand-web/backlog/phase-0/issues/README.md)
Phase plan: [`phase-0-discovery-and-gates.md`](../markhand-web/phase-0-discovery-and-gates.md)
Status: Done

## Objective

Chốt chunking/hybrid parameters và canonical signature.

## Context

- Phase: `0`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> identity schema v2 + expected-chunks + golden `chunkId` fill;
> neural hybrid (AITeamVN CPU / `local-neural`) on `local-cpu-quality`; frozen RRF
> `VECTOR_WEIGHT=0.55`; version-citation P/R scored as top-k `(doc,version,chunkId)`;
> temporal/change/conflict gates via deterministic offline rules. Closes on P0-05
> CPU quality evidence (ADR 0005 may remain Proposed until product model acceptance);
> does not require Profile B / vLLM cutover.

## Implementation plan

So chunk sizes; FTS/vector/hybrid; tune RRF; định nghĩa length-delimited
signature gồm model/revision/dim/normalize/chunk/text-normalization version;
version-aware identity và query modes current/as-of/compare/history; chuẩn hoá typed
claim key/value/unit/scope/effective interval và deterministic numeric/enum/date/
MUST-vs-MUST-NOT conflict candidates.

## Files/modules

`bench/markhand_web/retrieval/`, `expected-chunks.tsv`,
`reports/retrieval-evaluation.md`, ADR index signature.

## Dependencies / blocks

Desktop baseline + embedding result.

## Acceptance criteria

So sánh identical candidates; source span vẫn resolve; signature
test vector ổn định; chunk ID có document-version; temporal/current accuracy và
version-citation precision/recall đạt gate; conflict precision/recall, current-warning
và resolved-history accuracy có cited evidence.

## Required tests / evidence

Recall/MRR/nDCG/citation/no-answer + variance; cross-run signature.

## Security and migration notes

Signature đổi tạo index generation mới, không trộn vector.

## Out of scope

Server adapters/ACL.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- UNKNOWN — no completion/evidence commit is cited in the catalog status.

- GitHub sync-closed timestamp: `2026-07-18T18:55:39Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
