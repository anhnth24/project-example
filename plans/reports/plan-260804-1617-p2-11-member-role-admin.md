<!-- generated-done-issue-plan: P2-11 -->
# P2-11 — Member/role admin

Date: 2026-08-04
Source issue: [#126](https://github.com/anhnth24/project-example/issues/126)
Catalog: [`backlog/phase-2/issues/README.md`](../markhand-web/backlog/phase-2/issues/README.md)
Phase plan: [`phase-2-web-spa.md`](../markhand-web/phase-2-web-spa.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Member/role admin**.

## Context

- Phase: `2`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> #317. UI member table/invite (one-time token)/suspend/role/remove, owner-tier fail-closed mirror server, last-owner 409 + owner-tier 403 mapped. Mở khoá nhờ lát membership API (1C-02/1C-11) landed cùng #317.

## Implementation plan

Member table/invite/suspend/role selector; owner restrictions from API.

## Files/modules

The source catalog records implementation and file scope together; see **Implementation plan** above.

## Dependencies / blocks

P2-02/03/05 + backend 1C-02…04.

## Acceptance criteria

Owner/admin matrix,
last-owner conflict, invite/suspend/role/403/409/stale-update tests.

## Required tests / evidence

Owner/admin matrix,
last-owner conflict, invite/suspend/role/403/409/stale-update tests.

## Security and migration notes

UI không hard-code matrix hay thay enforcement.

## Out of scope

custom/group/SSO.

## Delivery evidence

### Implementation PRs

- [PR #317](https://github.com/anhnth24/project-example/pull/317) — Membership admin, end to end: server API + web UI (P2-11, P2-12; 1C-02/1C-11 slice); merged `2026-07-28T06:34:53Z`

### Recorded commit/SHA references

- `64b80d47fecad7d28c4a2b2df2422a892d56e46b`

- GitHub sync-closed timestamp: `2026-07-28T07:50:02Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
