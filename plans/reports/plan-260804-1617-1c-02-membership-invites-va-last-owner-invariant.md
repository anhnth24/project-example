<!-- generated-done-issue-plan: 1C-02 -->
# 1C-02 — Membership, invites và last-owner invariant

Date: 2026-08-04
Base commit: UNKNOWN — not recorded in the source catalog
Source issue: [#103](https://github.com/anhnth24/project-example/issues/103)
Catalog: [`backlog/phase-1c/issues/README.md`](../markhand-web/backlog/phase-1c/issues/README.md)
Phase plan: [`phase-1c-multi-org-security.md`](../markhand-web/phase-1c-multi-org-security.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Membership, invites và last-owner invariant**.

## Context

- Phase: `1C`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> (run [30629207747](https://github.com/anhnth24/project-example/actions/runs/30629207747)):
> `rust` [91151657403](https://github.com/anhnth24/project-example/actions/runs/30629207747/job/91151657403),
> `web` [91151657388](https://github.com/anhnth24/project-example/actions/runs/30629207747/job/91151657388),
> `rust-integration` [91151657399](https://github.com/anhnth24/project-example/actions/runs/30629207747/job/91151657399)
> (not path-filter skip / soft pass). Integration log executed `members` binary tests
> including `concurrent_last_owner_race_exactly_one_survives`,
> `cross_org_denial_covers_every_member_endpoint`, and
> `member_manage_permission_required_for_patch_and_delete`; job summary reports no
> ignored tests for that binary. Implementation landed in #317+: hashed single-use
> invites, PATCH/DELETE members, transactional last-owner + owner-tier guards,
> suspend/reactivate, session family revoke, audit allowlist. Membership ACL
> `version` remains deferred to 1C-05; automated email delivery remains out of scope.

## Implementation plan

Hashed single-use invite; membership state; transactional last-owner;
membership version (deferred to 1C-05); session revoke. MVP chưa có mail dùng invite
URL/token hiển thị đúng một lần cho admin copy qua kênh được tổ chức phê duyệt;
expiry/revoke/audit bắt buộc — **đã landed**.

## Files/modules

The source catalog records implementation and file scope together; see **Implementation plan** above.

## Dependencies / blocks

1C-01.

## Acceptance criteria

Không remove/downgrade last owner (kể cả tự
thao tác chính mình); admin không quản owner; concurrent owner removal, invite
replay/expiry, escalation tests — green on CI
`a62850422dd070e7e1195bfe1d4f1dee0d73566d` run
[30629207747](https://github.com/anhnth24/project-example/actions/runs/30629207747)
(`tests/members.rs` in `rust-integration`).

## Required tests / evidence

Không remove/downgrade last owner (kể cả tự
thao tác chính mình); admin không quản owner; concurrent owner removal, invite
replay/expiry, escalation tests — green on CI
`a62850422dd070e7e1195bfe1d4f1dee0d73566d` run
[30629207747](https://github.com/anhnth24/project-example/actions/runs/30629207747)
(`tests/members.rs` in `rust-integration`).

## Security and migration notes

Row lock (`FOR UPDATE` trên owner rows), expand/backfill
version (deferred to 1C-05); plaintext invite không lưu DB/log (chỉ trả 1 lần trong

## Out of scope

automated email delivery/SCIM/MFA.

## Delivery evidence

### Implementation PRs

- [PR #317](https://github.com/anhnth24/project-example/pull/317) — Membership admin, end to end: server API + web UI (P2-11, P2-12; 1C-02/1C-11 slice); merged `2026-07-28T06:34:53Z`

### Completion/evidence commits

- `30629207747`
- `64b80d47fecad7d28c4a2b2df2422a892d56e46b`
- `91151657388`
- `91151657399`
- `91151657403`
- `a62850422dd070e7e1195bfe1d4f1dee0d73566d`

- GitHub sync-closed timestamp: `2026-08-01T00:18:31Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
