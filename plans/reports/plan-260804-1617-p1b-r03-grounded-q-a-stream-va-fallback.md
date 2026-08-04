<!-- generated-done-issue-plan: P1B-R03 -->
# P1B-R03 — Grounded Q&A, stream và fallback

Date: 2026-08-04
Source issue: [#93](https://github.com/anhnth24/project-example/issues/93)
Catalog: [`backlog/phase-1b/issues/README.md`](../markhand-web/backlog/phase-1b/issues/README.md)
Phase plan: [`phase-1b-single-org-poc.md`](../markhand-web/phase-1b-single-org-poc.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Grounded Q&A, stream và fallback**.

## Context

- Phase: `1B`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> acceptance matrix green on CI rust-integration (`b5cc92c`,
> run [30603158015](https://github.com/anhnth24/project-example/actions/runs/30603158015)/job
> [91070008980](https://github.com/anhnth24/project-example/actions/runs/30603158015/job/91070008980)):
> full `ask_grounding_matrix` passed. Ask remains intentionally fail-closed /
> extractive when structured entailment is unavailable — **does not** claim
> structured-entailment or GLM grounded. Conflict hydrate exposes
> status/resolutionNote; current warns only `open`; history emits resolution
> notes for resolved/accepted_exception/false_positive. Prior live evidence
> retained: delete/ACL-revoke mid-stream barriers
> (`live_ask_stream_slow_trickle_concurrent_delete_releases_locks`,
> `live_ask_stream_jwt_exp_membership_and_delete_barriers`);
> `live_ask_conflict_triage_then_current_and_history_matrix`;
> `live_ask_wrong_delta_and_contradiction_soak_stays_fail_closed`.
> `STRUCTURED_ENTAILMENT_AVAILABLE = false` / `force_extractive_only()` stay
> hardcoded; opt-in `MARKHAND_QA_ALLOW_UNVERIFIED_LLM` (default OFF) may emit
> `llm_unverified` with fixed warning, never grounded.

## Implementation plan

Policy-separated prompt, untrusted passage framing, GLM, version-aware
citation validation, current answer + history/change note, token stream,
current unresolved-conflict warnings + resolved-history note, token stream,
deterministic extractive fallback.

## Files/modules

`services/qa/{mod,prompt,provider,grounding,stream}.rs`,
`services/stream_auth.rs`, `routes/ask.rs`, `tests/ask_grounding_matrix.rs`.

## Dependencies / blocks

R01/R02 + G0-RET/G0-SEC/G1A.

## Acceptance criteria

Citation subset only; current claim không cite version cũ;
compare cite old+new và đúng delta; injection không tool/scope change; provider
outage fallback; BA/design numeric conflict warning và v2 resolution; false-positive/
accepted-exception; fabricated/version-mix/conflict citation, timeout,
delete-during-stream tests.

## Required tests / evidence

Citation subset only; current claim không cite version cũ;
compare cite old+new và đúng delta; injection không tool/scope change; provider
outage fallback; BA/design numeric conflict warning và v2 resolution; false-positive/
accepted-exception; fabricated/version-mix/conflict citation, timeout,
delete-during-stream tests.

## Security and migration notes

Audit metadata only.

## Out of scope

agents/memory/web browse.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- `b5cc92c`

- GitHub sync-closed timestamp: `2026-07-31T06:23:23Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
