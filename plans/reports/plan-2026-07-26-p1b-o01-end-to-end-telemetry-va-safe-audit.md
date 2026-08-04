<!-- generated-done-issue-plan: P1B-O01 -->
# P1B-O01 — End-to-end telemetry và safe audit

Issue closed: 2026-07-26
Source issue: [#97](https://github.com/anhnth24/project-example/issues/97)
Catalog: [`backlog/phase-1b/issues/README.md`](../markhand-web/backlog/phase-1b/issues/README.md)
Phase plan: [`phase-1b-single-org-poc.md`](../markhand-web/phase-1b-single-org-poc.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **End-to-end telemetry và safe audit**.

## Context

- Phase: `1B`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> live evidence 2026-07-26 at `f4f33cd`: `o01-telemetry.json`
> `status=pass` with 0 blockers. The async API→worker→provider canary closed all
> 16 proofs (job terminal + payload `request_id`, DB audit row per request,
> exact deny audit, same-trace ingest and ask exports with the required
> `api.request`/`worker.convert`/`worker.index`/`worker.embed`/`retrieval`/
> `provider.chat` spans, unique span ids, canonical OTLP kinds, valid parent
> graph, grounded ask, clean metrics with no canary or high-cardinality label).
> Cargo telemetry suite, OTLP capture unit tests, live app-role audit test and
> the negative proof fixtures all passed. Report sha256 prefix
> `e8efc7b6975fdb4b`.

## Implementation plan

Traces API→jobs→convert/embed/retrieval/GLM; latency/queue/conversion/
embedding/retrieval/drift/quota/backup metrics; append-only audit.

## Files/modules

`src/telemetry/**`, `services/audit.rs`, `db/audit.rs`,
`deploy/dev/otel-collector.yaml`.

## Dependencies / blocks

F01/F05/I03 + G0-SLO.

## Acceptance criteria

Correlation qua async; action/deny coverage; canary secret/
content absent; trace/cardinality/redaction/audit tests.

## Required tests / evidence

Correlation qua async; action/deny coverage; canary secret/
content absent; trace/cardinality/redaction/audit tests.

## Security and migration notes

Allowlist log fields.

## Out of scope

SIEM.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- `e8efc7b6975fdb4b`
- `f4f33cd`

- GitHub sync-closed timestamp: `2026-07-26T14:40:12Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
