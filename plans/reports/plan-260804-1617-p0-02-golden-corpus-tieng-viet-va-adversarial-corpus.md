<!-- generated-done-issue-plan: P0-02 -->
# P0-02 — Golden corpus tiếng Việt và adversarial corpus

Date: 2026-08-04
Base commit: UNKNOWN — not recorded in the source catalog
Source issue: [#59](https://github.com/anhnth24/project-example/issues/59)
Catalog: [`backlog/phase-0/issues/README.md`](../markhand-web/backlog/phase-0/issues/README.md)
Phase plan: [`phase-0-discovery-and-gates.md`](../markhand-web/phase-0-discovery-and-gates.md)
Status: Done

## Objective

Dataset tái lập cho conversion, retrieval, citation và upload attack.

## Context

- Phase: `0`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> strict reproducibility gates passed.

## Implementation plan

Thêm mọi format; 200–500 query với expected document/source span/
relevance/no-answer; multi-document và immutable multi-version citations
(`current`/`as_of`/`compare`/`history`); BA/design/dev cross-document conflict
lifecycle (open/resolved/history) với cited claims; sample spoof/bomb/malformed/
traversal/prompt injection; pin checksum và provenance/license.

## Files/modules

`bench/markhand_web/golden/`, `adversarial/`,
`manifest.lock.json`, `scripts/validate_corpus.py`.

## Dependencies / blocks

P0-01; fixture phải redistributable.

## Acceptance criteria

Coverage đủ category; source/version span ổn định; current fact không
trỏ version cũ; compare/history cite đủ old+new và delta; conflict current/history
cite đủ hai phía + resolution versions; validator bắt checksum, duplicate ID, invalid
span/version/conflict lineage và missing license; mỗi attack có expected disposition.

## Required tests / evidence

Clean-checkout reproducibility; dual review + adjudication.

## Security and migration notes

Synthetic/de-identified; bomb fixture chỉ chạy trong limits.

## Out of scope

Customer data và expected chunk ID trước khi chốt chunking.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Completion/evidence commits

- UNKNOWN — no completion/evidence commit is cited in the catalog status.

- GitHub sync-closed timestamp: `2026-07-18T17:05:50Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
