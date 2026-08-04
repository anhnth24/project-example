<!-- generated-done-issue-plan: 1C-06 -->
# 1C-06 — PostgreSQL ACL enforcement

Date: 2026-08-04
Source issue: [#108](https://github.com/anhnth24/project-example/issues/108)
Catalog: [`backlog/phase-1c/issues/README.md`](../markhand-web/backlog/phase-1c/issues/README.md)
Phase plan: [`phase-1c-multi-org-security.md`](../markhand-web/phase-1c-multi-org-security.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **PostgreSQL ACL enforcement**.

## Context

- Phase: `1C`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> CI exact-SHA evidence on `90742281e51d3c8ca8a32a78077a07fe3449bc68`
> (run [30649044974](https://github.com/anhnth24/project-example/actions/runs/30649044974)):
> changes/static
> [91217513655](https://github.com/anhnth24/project-example/actions/runs/30649044974/job/91217513655),
> `rust`
> [91217686329](https://github.com/anhnth24/project-example/actions/runs/30649044974/job/91217686329),
> `rust-integration`
> [91217686352](https://github.com/anhnth24/project-example/actions/runs/30649044974/job/91217686352)
> (not path-filter skip / soft pass). Task 6 rereview Approved. Integration log executed
> PostgreSQL ACL enforcement targets including
> `fts_rank_accent_fold_and_active_generation_gates` (dual `qa.query` + `qa.history`
> hydration recheck), `fts_candidate_leg_and_hydration_deny_acl_and_suspended_membership`,
> upload regression quartet (`http_upload_happy_and_spoof`,
> `cancelled_http_upload_settles_quota_consistently`,
> `envelope_binds_collection_and_stable_replay_deep_equality`,
> `quarantined_review_requires_approval_for_single_job`), private grant rejection, and
> role-grant leave-groups. Shared `db/acl_sql` predicates on FTS/hydration/conflict paths;
> explicit resolver↔SQL equivalence oracle; upload/quarantine operation-scoped write guards
> (`doc.upload` / `doc.quarantine.review` at `AccessLevel::Write`, no read-projection
> inference).

## Implementation plan

Tenant+ACL predicates cho FTS/hydration/conflict (**done**); upload write
gate (**done**). Autocomplete không xây (out of scope — xem điểm 4 legacy note bên dưới).

## Files/modules

The source catalog records implementation and file scope together; see **Implementation plan** above.

## Dependencies / blocks

1C-05.

## Acceptance criteria

Không path thiếu context; no existence/count
leak; SQL join/subquery/missing-predicate tests — green on CI
`90742281e51d3c8ca8a32a78077a07fe3449bc68` run
[30649044974](https://github.com/anhnth24/project-example/actions/runs/30649044974)
(`retrieval`, `uploads`, fast unit pins in `db/search.rs` / `db/acl_sql.rs`).

## Required tests / evidence

Không path thiếu context; no existence/count
leak; SQL join/subquery/missing-predicate tests — green on CI
`90742281e51d3c8ca8a32a78077a07fe3449bc68` run
[30649044974](https://github.com/anhnth24/project-example/actions/runs/30649044974)
(`retrieval`, `uploads`, fast unit pins in `db/search.rs` / `db/acl_sql.rs`).

## Security and migration notes

PG authority, prepared queries; migration `0036` dormant-grant
rejection enforced at seed + runtime.

## Out of scope

vector/object path (1C-07), autocomplete,
broader route write inventory (PR 3).

### Legacy verification notes (pre-PR-2, retained for audit trail)

1. **FTS candidate ACL subquery** (`db/search.rs`): defense-in-depth EXISTS on candidate
leg; shared builder includes `acl_m.state = 'active'`.
2. **Org-only count helpers** (`db/documents::count`, `db/chunks::count`): test-only;
`org_only_count_helpers_stay_out_of_routes_and_services` source-scan guard.
3. **Missing-predicate pin**: `every_chunk_scoped_query_embeds_acl_predicate` +
`acl_predicate_sql_shape_is_pinned`.
4. **Autocomplete**: không có endpoint — future work must use `acl_predicate_sql` from day one.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- `90742281e51d3c8ca8a32a78077a07fe3449bc68`

- GitHub sync-closed timestamp: `2026-08-01T00:18:40Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
