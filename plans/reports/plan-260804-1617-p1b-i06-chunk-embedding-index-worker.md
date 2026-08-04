<!-- generated-done-issue-plan: P1B-I06 -->
# P1B-I06 — Chunk/embedding/index worker

Date: 2026-08-04
Base commit: UNKNOWN — not recorded in the source catalog
Source issue: [#89](https://github.com/anhnth24/project-example/issues/89)
Catalog: [`backlog/phase-1b/issues/README.md`](../markhand-web/backlog/phase-1b/issues/README.md)
Phase plan: [`phase-1b-single-org-poc.md`](../markhand-web/phase-1b-single-org-poc.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Chunk/embedding/index worker**.

## Context

- Phase: `1B`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> `lifecycle_refresh` (one idempotent job per materialized generation; no
> active-generation fallback); Index↔LifecycleRefresh claim fairness
> (ConvertWorker atomic pattern); mixed-scope filter-only Qdrant update (has_id
> + org/collection/version, no body `points`). LiveEnv dual-role
> (`markhand_app`). Local:
> `cargo test -p fileconv-server --test index_worker -- --include-ignored`
> → 10 ok (natural A→B, multi-gen demote + idempotent replay, fairness ≤2
> `run_once`, mixed-scope, race, retry).

## Implementation plan

Core chunking + knowledge identity/signature chứa `version_id`; PG
chunks/FTS; separate embedding batches; Qdrant payload version/effective/current;
extract typed claim key/value/unit/scope; incremental conflict candidate outbox;
blocking client off async executor; deterministic upsert.

## Files/modules

`workers/{index,embedding}.rs`, `services/{chunking,embedding,indexing}.rs`.

## Dependencies / blocks

I03/I05/F06 + G0-RET/G0-CAP/G1A.

## Acceptance criteria

Approved signature; ≤1 replay batch; no duplicate; mismatch
before publish; golden/mock/backpressure/kill/consistency tests;
`live_index_worker_replay_is_idempotent`;
`live_index_worker_stale_version_does_not_mark_current_indexed`.

## Required tests / evidence

Approved signature; ≤1 replay batch; no duplicate; mismatch
before publish; golden/mock/backpressure/kill/consistency tests;
`live_index_worker_replay_is_idempotent`;
`live_index_worker_stale_version_does_not_mark_current_indexed`.

## Security and migration notes

Local approved embedding only; new signature=new generation.

## Out of scope

user-selected models.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Completion/evidence commits

- UNKNOWN — no completion/evidence commit is cited in the catalog status.

- GitHub sync-closed timestamp: `2026-07-20T08:14:12Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
