<!-- generated-done-issue-plan: P0-08 -->
# P0-08 — Sizing converter và ingest backpressure

Date: 2026-08-04
Source issue: [#65](https://github.com/anhnth24/project-example/issues/65)
Catalog: [`backlog/phase-0/issues/README.md`](../markhand-web/backlog/phase-0/issues/README.md)
Phase plan: [`phase-0-discovery-and-gates.md`](../markhand-web/phase-0-discovery-and-gates.md)
Status: Done

## Objective

Chốt worker count, limits, timeout, queue và recovery headroom.

## Context

- Phase: `0`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> interim local-cpu sizing harness/report closes P0-08
> deliverables with `targetMatch=false`; Profile B `G0-CAP-INGEST-THROUGHPUT`
> and production headroom remain blocked until measured on `on-prem-reference`.

## Implementation plan

Benchmark từng format native/scan/audio; single/concurrent; CPU/RAM/temp;
PDFium serialization; converter-vs-GPU bottleneck.

## Files/modules

`bench/markhand_web/ingest/`, `scripts/run_ingest_capacity.sh`,
`reports/ingest-capacity.md`.

## Dependencies / blocks

Golden files + native deps available for local-cpu smoke;
production capacity remains blocked by Profile B hardware.

## Acceptance criteria

Mọi POC format có sizing/timeout and simulated queue-age evidence;
≥30% production resource headroom is not claimed from local-cpu.

## Required tests / evidence

Harness self-test + full local-cpu run writes
`bench/markhand_web/ingest/summary.json` and
`bench/markhand_web/reports/ingest-capacity.md`; rerun on Profile B for
gate pass evidence.

## Security and migration notes

Malformed input chỉ chạy dưới limits.

## Out of scope

Production job engine.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- UNKNOWN — no completion/evidence commit is cited in the catalog status.

- GitHub sync-closed timestamp: `2026-07-18T19:23:11Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
