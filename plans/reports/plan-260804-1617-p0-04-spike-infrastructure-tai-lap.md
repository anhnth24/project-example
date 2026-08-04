<!-- generated-done-issue-plan: P0-04 -->
# P0-04 — Spike infrastructure tái lập

Date: 2026-08-04
Source issue: [#61](https://github.com/anhnth24/project-example/issues/61)
Catalog: [`backlog/phase-0/issues/README.md`](../markhand-web/backlog/phase-0/issues/README.md)
Phase plan: [`phase-0-discovery-and-gates.md`](../markhand-web/phase-0-discovery-and-gates.md)
Status: Done

## Objective

Stack disposable PG/Qdrant/MinIO/vLLM/telemetry cho benchmark.

## Context

- Phase: `0`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> reproducible stack, pinned images, three-store lifecycle and bound
> CPU-smoke evidence passed; Profile B GPU/IOPS measurements remain downstream gates.

## Implementation plan

Tái dùng compose/services/scripts base từ F-08; thêm benchmark-specific
override với isolated volumes/data, vLLM/GPU profile, workload sizing, image digest
và environment fingerprint. Không fork dev stack.

## Files/modules

`deploy/compose.spike.yml`, `deploy/spike/`, base `deploy/dev/`,
`bench/markhand_web/scripts/spike-{health,reset}.sh`.

## Dependencies / blocks

Phase F/F-08 + P0-01; target hardware để đóng issue.

## Acceptance criteria

Một command boot từ empty volumes; không thao tác console; restart/
reset đúng semantics.

## Required tests / evidence

Clean-machine startup, service health, version/GPU/telemetry
fingerprint.

## Security and migration notes

Bind private/localhost; non-production secrets ngoài Git.

## Out of scope

HA/TLS/production orchestration.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- UNKNOWN — no completion/evidence commit is cited in the catalog status.

- GitHub sync-closed timestamp: `2026-07-18T17:05:54Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
