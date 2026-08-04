<!-- generated-done-issue-plan: P0-03 -->
# P0-03 — Mở rộng desktop baseline trên corpus Phase 0

Date: 2026-08-04
Base commit: UNKNOWN — not recorded in the source catalog
Source issue: [#60](https://github.com/anhnth24/project-example/issues/60)
Catalog: [`backlog/phase-0/issues/README.md`](../markhand-web/backlog/phase-0/issues/README.md)
Phase plan: [`phase-0-discovery-and-gates.md`](../markhand-web/phase-0-discovery-and-gates.md)
Status: Done

## Objective

Mở rộng parity baseline P1A-01 lên corpus/metrics Phase 0; P1A-01 là
baseline authoritative để việc extraction không phải đợi toàn bộ corpus.

## Context

- Phase: `0`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> recomputed evidence accepted as the current-state reference.

## Implementation plan

Tái dùng fixtures/harness P1A-01; chạy release conversion; snapshot top-k,
scores, anchors, answer modes,
warnings, stats, provider fallback và signature mismatch.

## Files/modules

`bench/markhand_web/scripts/run_desktop_baseline.sh`,
`baselines/desktop-v1/`, `reports/desktop-baseline.md`.

## Dependencies / blocks

P0-02 + P1A-01 authoritative parity harness; provider run
cần config/model pin.

## Acceptance criteria

Mọi format/query có raw machine-readable result; offline chạy không
cần LLM; đủ dữ liệu so parity 1A.

## Required tests / evidence

CER/WER/time, Recall@5/10, MRR, nDCG, citation correctness;
deterministic rerun.

## Security and migration notes

Redact key/prompt/absolute path.

## Out of scope

Sửa defect ranking/performance.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Completion/evidence commits

- UNKNOWN — no completion/evidence commit is cited in the catalog status.

- GitHub sync-closed timestamp: `2026-07-18T17:05:52Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
