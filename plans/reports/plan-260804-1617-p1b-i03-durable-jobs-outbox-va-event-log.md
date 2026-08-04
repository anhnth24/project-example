<!-- generated-done-issue-plan: P1B-I03 -->
# P1B-I03 — Durable jobs, outbox và event log

Date: 2026-08-04
Source issue: [#86](https://github.com/anhnth24/project-example/issues/86)
Catalog: [`backlog/phase-1b/issues/README.md`](../markhand-web/backlog/phase-1b/issues/README.md)
Phase plan: [`phase-1b-single-org-poc.md`](../markhand-web/phase-1b-single-org-poc.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Durable jobs, outbox và event log**.

## Context

- Phase: `1B`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> Catalog records status as Done.

## Implementation plan

Versioned payload, transactional outbox, leased SKIP LOCKED claims,
heartbeat/retry/checkpoint/cancel/dead-letter/idempotency/sequenced events.

## Files/modules

`src/jobs/**`, `src/db/jobs.rs`.

## Dependencies / blocks

F03/F04 + G0-CAP.

## Acceptance criteria

Commit/enqueue không split; lease reclaimed; duplicate harmless;
kill/checkpoint/claim/dead-letter/cancel/outbox replay.

## Required tests / evidence

Commit/enqueue không split; lease reclaimed; duplicate harmless;
kill/checkpoint/claim/dead-letter/cancel/outbox replay.

## Security and migration notes

IDs only, no content/secrets; backward-readable payloads.

## Out of scope

Kafka/Redis queue.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- UNKNOWN — no completion/evidence commit is cited in the catalog status.

- GitHub sync-closed timestamp: `2026-07-19T15:17:29Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
