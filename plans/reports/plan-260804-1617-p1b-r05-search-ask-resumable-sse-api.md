<!-- generated-done-issue-plan: P1B-R05 -->
# P1B-R05 — Search/ask/resumable SSE API

Date: 2026-08-04
Source issue: [#95](https://github.com/anhnth24/project-example/issues/95)
Catalog: [`backlog/phase-1b/issues/README.md`](../markhand-web/backlog/phase-1b/issues/README.md)
Phase plan: [`phase-1b-single-org-poc.md`](../markhand-web/phase-1b-single-org-poc.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Search/ask/resumable SSE API**.

## Context

- Phase: `1B`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> full `sse_stream_readiness` matrix green on CI
> rust-integration (`b5cc92c`, run
> [30603158015](https://github.com/anhnth24/project-example/actions/runs/30603158015)/job
> [91070008980](https://github.com/anhnth24/project-example/actions/runs/30603158015/job/91070008980)).
> Implemented: ask/job reserve-before-select on cap-1 channel; family→principal→
> fresh OrgContext → select ≤1 event under fixed pull deadline; production
> `/auth/logout` router barriers; concurrent delete trickle + `acl_mutate`
> role/collection barriers assert no new sequenced content after commit;
> delayed-producer reconnect
> (`live_ask_stream_last_event_id_purge_and_delayed_reconnect`) and
> purge/load bound
> (`live_ask_stream_maintenance_converges_under_bounded_load`) evidence.
> Production ask remains fail-closed extractive when entailment is unavailable
> (by design — see P1B-R03; not a Done blocker).

## Implementation plan

Search/ask/stream routes; versioned sequence; Last-Event-ID replay;
heartbeat/bounded buffering; auth expiry/revoke close.

## Files/modules

`routes/{search,ask,events}.rs`, `api/{sse,last_event_id}.rs`,
`db/ask_streams.rs`, `services/qa/{ask_stream,provider,stream}.rs`,
`services/stream_auth.rs`,
`migrations/0024_expand_ask_stream_sessions.sql`,
`migrations/0025_backfill_event_log_ids_ask_stream_ops.sql`.

## Dependencies / blocks

F05/I03/R01/R03/R04.

## Acceptance criteria

No lost acknowledged/duplicate sequence; bounded slow client;
reconnect/order/expiry/revoke/worker restart; zero post-revoke content; durable
terminal/control; Last-Event-ID validation; retention purge; provider framing;
lifecycle lease/recovery.

## Required tests / evidence

No lost acknowledged/duplicate sequence; bounded slow client;
reconnect/order/expiry/revoke/worker restart; zero post-revoke content; durable
terminal/control; Last-Event-ID validation; retention purge; provider framing;
lifecycle lease/recovery.

## Security and migration notes

Scoped per user/org/job, no cache.

## Out of scope

WebSocket.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- `b5cc92c`

- GitHub sync-closed timestamp: `2026-07-31T06:23:28Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
