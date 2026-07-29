-- Phase: 1C
-- Owner: security-owner, storage-owner
-- Change: expand
-- Lock/data risk: single ADD COLUMN ... NOT NULL DEFAULT on `orgs`. Row count
--   is POC-scale (single-org POC seed plus whatever 1C-01 `POST /orgs` has
--   created since), so the ACCESS EXCLUSIVE lock taken for the DEFAULT
--   backfill is momentary. No shipped application code reads or writes this
--   column before this change.
-- Rollback compatibility: no released application version depends on
--   `acl_version`; dropping the column later is compatible with any released
--   client. `auth::context_cache::OrgContextCache` degrades to "never a
--   cache hit" (always falls through to a full resolve), not to an error, if
--   the freshness-check query against this column fails for any reason —
--   see that module's doc comment.
--
-- 1C-05: coarse, ORG-WIDE monotonic version used only to invalidate
-- `auth::context_cache`'s in-process `OrgContext` cache. Bumped by
-- `db::orgs::bump_acl_version` in the SAME transaction as any mutation that
-- can change what `auth::permissions::resolve_org_context*` computes for ANY
-- member of the org:
--   - `services::members::change_role` / `suspend_member` /
--     `reactivate_member` / `remove_member`
--   - `services::acl_mutate::revoke_role_permission_for_principal` /
--     `revoke_collection_access_for_principal`
--
-- Deliberately ORG-WIDE rather than per-membership or per-role:
-- `revoke_role_permission_for_principal` deletes a `role_permissions` row
-- that is shared by every member currently holding that role, not only the
-- named principal, so a narrower cache key would under-invalidate (unsafe —
-- another member holding the same role would keep a stale, wider permission
-- set). Over-invalidating the whole org on every one of these mutations is
-- the safe direction and is acceptable at the org sizes this POC operates
-- at; there is no per-org fairness/SLO concern filed against this (1C-10 is
-- separately still backlog for unrelated reasons).
--
-- KNOWN GAP (see session report for the full analysis): collection
-- create/soft-delete and any future direct visibility mutation in
-- `db::collections` (outside `services::acl_mutate`) do NOT bump this
-- column yet, because that call-site audit + its own test coverage is a
-- separate, larger piece of work than this migration's scope. Until closed,
-- staleness from that specific gap is bounded only by the cache's short TTL
-- (`auth::context_cache::DEFAULT_TTL`), not by immediate invalidation.

ALTER TABLE orgs
    ADD COLUMN acl_version bigint NOT NULL DEFAULT 1
        CHECK (acl_version > 0);

COMMENT ON COLUMN orgs.acl_version IS
    'Monotonic org-wide version bumped by any membership/ACL mutation that can change a resolved OrgContext; see auth::context_cache for the in-process cache it invalidates (1C-05).';
