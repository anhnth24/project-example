-- Phase: 1C
-- Owner: security-owner
-- Change: expand
-- Lock/data risk: single ADD COLUMN ... NOT NULL DEFAULT on org_memberships; POC
--   seed has only a handful of rows (single-org POC, migrations/0011), so the
--   ACCESS EXCLUSIVE lock taken for the DEFAULT backfill is momentary. No shipped
--   application code reads or writes this column yet.
-- Rollback compatibility: no released application version depends on `state`;
--   dropping the column later is compatible with any released client.
--
-- P2-11 needs "suspend member" to be distinct from "remove member": a suspended
-- membership keeps its row (role, created_at, audit trail, owner-history) but
-- must be treated as absent for authorization. `active` is the default so every
-- existing/POC-seeded row stays authorized after this migration applies.
-- The org-context resolver (auth/permissions.rs resolve_org_context /
-- resolve_org_context_on_txn) is updated in the same change to require
-- state = 'active', so a suspended member resolves exactly like a missing
-- membership (MembershipMissing), fail-closed.
--
-- Deferred on purpose: a `version` column for 1C-05 ACL cache invalidation is
-- NOT added here (see plans/reports/plan-260728-0231-markhand-web-membership-admin-slice.md
-- section 6/8b) — there is no cache yet to invalidate.

ALTER TABLE org_memberships
    ADD COLUMN state text NOT NULL DEFAULT 'active'
        CHECK (state IN ('active', 'suspended'));

COMMENT ON COLUMN org_memberships.state IS
    'active|suspended. Suspended members resolve as MembershipMissing (P2-11 suspend, distinct from DELETE); row/history is preserved.';
