<!-- generated-done-issue-plan: 1C-05 -->
# 1C-05 — Collection ACL resolver/cache

Issue closed: 2026-08-01
Source issue: [#107](https://github.com/anhnth24/project-example/issues/107)
Catalog: [`backlog/phase-1c/issues/README.md`](../markhand-web/backlog/phase-1c/issues/README.md)
Phase plan: [`phase-1c-multi-org-security.md`](../markhand-web/phase-1c-multi-org-security.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Collection ACL resolver/cache**.

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
> (not path-filter skip / soft pass). Task 4 review Approved; Task 5 review Approved.
> Integration log executed ACL resolver/cache targets including
> `groups_visibility_group_grant_allows_member_without_user_grant`,
> `private_visibility_ignores_group_and_role_grants`,
> `containment_removes_group_role_grants_but_preserves_other_user_grants`,
> `resolver_matches_sql_predicate_for_acl_fixture_matrix`,
> `read_grant_does_not_satisfy_write_or_admin`,
> `group_membership_revoke_invalidates_cached_context`, concurrent grant-vs-flip, and
> ACL-version bump. Canonical `(qa.query, read)` resolver projection via
> `allowed_collections_sql` with `private`/`org`/`groups` visibility + group/role grant
> branches; migration `0036` dormant-grant rejection; org-wide `acl_version` cache
> invalidation (migrations `0031`/`0033`); `auth::context_cache` freshness check on
> extractor hits.

## Implementation plan

Private/org/groups grants (**done**); ACL/version
snapshot (**done**: `orgs.acl_version`, migration `0031`); cache key org/user/membership/
ACL version (**done**: `auth::context_cache::OrgContextCache`, key `(org_id, user_id)` +
version check); invalidation APIs (**không làm riêng** — invalidation là version-bump
trong transaction mutation, không phải API endpoint mới; không có yêu cầu nào đòi một API
invalidation tách biệt).

## Files/modules

The source catalog records implementation and file scope together; see **Implementation plan** above.

## Dependencies / blocks

1C-02/03.

## Acceptance criteria

Semantics đúng, empty/error fail closed;
grants/status/cache/revoke tests — green on CI
`90742281e51d3c8ca8a32a78077a07fe3449bc68` run
[30649044974](https://github.com/anhnth24/project-example/actions/runs/30649044974)
(`acl_resolver`, `acl_equivalence`, `acl_cache` in `rust-integration`).

## Required tests / evidence

Semantics đúng, empty/error fail closed;
grants/status/cache/revoke tests — green on CI
`90742281e51d3c8ca8a32a78077a07fe3449bc68` run
[30649044974](https://github.com/anhnth24/project-example/actions/runs/30649044974)
(`acl_resolver`, `acl_equivalence`, `acl_cache` in `rust-integration`).

## Security and migration notes

Backfill ACL version (**done**: `DEFAULT 1`, expand-only,
migration `0031`). **Gap version-bump đã ĐÓNG (migration `0033`, 2026-07-29)**: trigger
DB `bump_org_acl_version()` trên `collections`/`collection_user_access`/
`org_memberships`/`roles`/`role_permissions` — bump cùng transaction cho MỌI writer,
kể cả SQL trực tiếp (fixtures/vận hành; CI `rust-integration` bắt được đúng lỗ này

## Out of scope

nested/time-based groups; operator-configurable cache capacity/TTL qua env; collection
insert/soft-delete version bump outside `acl_mutate` (accepted TTL-bound gap).

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- `90742281e51d3c8ca8a32a78077a07fe3449bc68`

- GitHub sync-closed timestamp: `2026-08-01T00:18:38Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
