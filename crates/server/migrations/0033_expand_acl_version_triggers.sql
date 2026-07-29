-- Phase: 1C
-- Owner: security-owner
-- Change: expand
-- Lock/data risk: two row-level triggers + one trigger function on existing
--   tables; no table rewrite, no data touched. Trigger fires one indexed
--   UPDATE on orgs per mutated row — collection/ACL mutations are low-volume
--   admin operations, not hot paths.
-- Rollback compatibility: additive only; dropping the triggers/function in a
--   later contract migration restores 0031's application-side-only bumping.
--
-- 1C-05 follow-up: close the declared "collection create/soft-delete does not
-- bump acl_version" gap — at the DATABASE layer, not per call site — and
-- extend the same guarantee to every other per-org table the resolver reads
-- (org_memberships, roles, role_permissions). users.disabled_at needs no
-- trigger: the cache re-checks it fresh on every hit already.
--
-- Why a trigger and not more application call sites: the org-context cache
-- (auth/context_cache.rs) revalidates every hit against orgs.acl_version, so
-- ANY writer that changes what the resolver would return must bump it in the
-- same transaction. Application call sites only cover application writers —
-- test fixtures and operational SQL seed collections/collection_user_access
-- directly (api_http_contracts::seed_published_doc, citation_authz_matrix's
-- ACL matrices, caught on CI), and those writers can never be enumerated in
-- Rust. A row-level trigger makes the invariant hold for every writer by
-- construction. This is deliberately different from the rejected
-- AFTER INSERT ON orgs role-seeding trigger (migration 0030's design note):
-- bumping a counter grants nothing and cannot flip a deny fixture to allow.

CREATE OR REPLACE FUNCTION bump_org_acl_version() RETURNS trigger AS $$
DECLARE
    target_org uuid;
BEGIN
    target_org := COALESCE(NEW.org_id, OLD.org_id);
    IF target_org IS NOT NULL THEN
        UPDATE orgs SET acl_version = acl_version + 1 WHERE id = target_org;
    END IF;
    RETURN COALESCE(NEW, OLD);
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS collections_bump_acl_version ON collections;
CREATE TRIGGER collections_bump_acl_version
    AFTER INSERT OR UPDATE OR DELETE ON collections
    FOR EACH ROW EXECUTE FUNCTION bump_org_acl_version();

DROP TRIGGER IF EXISTS collection_user_access_bump_acl_version ON collection_user_access;
CREATE TRIGGER collection_user_access_bump_acl_version
    AFTER INSERT OR UPDATE OR DELETE ON collection_user_access
    FOR EACH ROW EXECUTE FUNCTION bump_org_acl_version();

DROP TRIGGER IF EXISTS org_memberships_bump_acl_version ON org_memberships;
CREATE TRIGGER org_memberships_bump_acl_version
    AFTER INSERT OR UPDATE OR DELETE ON org_memberships
    FOR EACH ROW EXECUTE FUNCTION bump_org_acl_version();

DROP TRIGGER IF EXISTS roles_bump_acl_version ON roles;
CREATE TRIGGER roles_bump_acl_version
    AFTER INSERT OR UPDATE OR DELETE ON roles
    FOR EACH ROW EXECUTE FUNCTION bump_org_acl_version();

DROP TRIGGER IF EXISTS role_permissions_bump_acl_version ON role_permissions;
CREATE TRIGGER role_permissions_bump_acl_version
    AFTER INSERT OR UPDATE OR DELETE ON role_permissions
    FOR EACH ROW EXECUTE FUNCTION bump_org_acl_version();
