<!-- generated-done-issue-plan: P0-01 -->
# P0-01 — Khóa workload, hardware và gate registry

Date: 2026-08-04
Source issue: [#58](https://github.com/anhnth24/project-example/issues/58)
Catalog: [`backlog/phase-0/issues/README.md`](../markhand-web/backlog/phase-0/issues/README.md)
Phase plan: [`phase-0-discovery-and-gates.md`](../markhand-web/phase-0-discovery-and-gates.md)
Status: Done

## Objective

Thay giả định scale/SLA bằng workload envelope, hardware profile và
gate schema được duyệt.

## Context

- Phase: `0`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> approved Profile B, numeric targets and fail-closed validators
> merged to `master`.

## Implementation plan

Ghi org/collection/document/vector, ingest/query/recovery load; CPU/RAM/
disk/GPU/network; tạo registry gồm metric, workload, threshold, command,
environment, approver và failure disposition.

## Files/modules

`bench/markhand_web/{README.md,workload-profile.yaml,gates.yaml}`,
`bench/markhand_web/environments/`, `docs/adr/README.md`.

## Dependencies / blocks

Cần input sản phẩm/vận hành; block mọi benchmark.

## Acceptance criteria

Normal/peak/recovery/aggregate load đầy đủ; mọi open decision có
owner; gate thiếu trường bị schema validator từ chối.

## Required tests / evidence

Validate YAML/schema; mọi report sau emit environment
fingerprint.

## Security and migration notes

Không ghi credential, hostname nội bộ hoặc tên khách hàng.

## Out of scope

Chọn model và tuyên bố đạt SLA.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- UNKNOWN — no completion/evidence commit is cited in the catalog status.

- GitHub sync-closed timestamp: `2026-07-18T17:05:48Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
