-- Phase: 1C
-- Owner: security-owner
-- Change: expand
-- Lock/data risk: four SQL functions (three visibility guards + replacement
--   bump_org_acl_version) and six row-level triggers on existing ACL tables;
--   preflight DO block scans collections/grants once at apply time.
--   Grant/visibility writers serialize on the parent collection row via
--   FOR NO KEY UPDATE under READ COMMITTED — low-volume admin paths only.
-- Rollback compatibility: additive only; dropping triggers/functions in a later
--   contract migration restores pre-0036 grant semantics.

-- Preflight: fail deterministically with sorted collection IDs when dormant
-- group/role grants already exist on non-groups collections. Never silently
-- delete or activate them.
DO $$
DECLARE
    bad_ids text;
BEGIN
    SELECT string_agg(bad.id::text, ', ' ORDER BY bad.id)
    INTO bad_ids
    FROM (
        SELECT DISTINCT c.id
        FROM collections c
        WHERE c.visibility <> 'groups'
          AND (
              EXISTS (
                  SELECT 1
                  FROM collection_group_access g
                  WHERE g.org_id = c.org_id
                    AND g.collection_id = c.id
              )
              OR EXISTS (
                  SELECT 1
                  FROM collection_role_access r
                  WHERE r.org_id = c.org_id
                    AND r.collection_id = c.id
              )
          )
    ) AS bad;
    IF bad_ids IS NOT NULL THEN
        RAISE EXCEPTION 'preflight: dormant group/role grants on non-groups collections: %', bad_ids;
    END IF;
END $$;

CREATE OR REPLACE FUNCTION enforce_collection_group_role_grant_visibility()
RETURNS trigger AS $$
DECLARE
    parent_visibility text;
BEGIN
    IF TG_OP = 'UPDATE' AND (
        OLD.org_id IS DISTINCT FROM NEW.org_id
        OR OLD.collection_id IS DISTINCT FROM NEW.collection_id
    ) THEN
        RAISE EXCEPTION 'group/role grant parent keys (org_id, collection_id) are immutable; delete and re-insert to retarget';
    END IF;

    SELECT visibility
    INTO parent_visibility
    FROM collections
    WHERE org_id = NEW.org_id AND id = NEW.collection_id
    FOR NO KEY UPDATE;

    IF parent_visibility IS NULL THEN
        RAISE EXCEPTION 'collection % not found in org %', NEW.collection_id, NEW.org_id;
    END IF;

    IF parent_visibility <> 'groups' THEN
        RAISE EXCEPTION 'group/role grants require collection visibility groups (got %)', parent_visibility;
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS collection_group_access_enforce_visibility ON collection_group_access;
CREATE TRIGGER collection_group_access_enforce_visibility
    BEFORE INSERT OR UPDATE ON collection_group_access
    FOR EACH ROW EXECUTE FUNCTION enforce_collection_group_role_grant_visibility();

DROP TRIGGER IF EXISTS collection_role_access_enforce_visibility ON collection_role_access;
CREATE TRIGGER collection_role_access_enforce_visibility
    BEFORE INSERT OR UPDATE ON collection_role_access
    FOR EACH ROW EXECUTE FUNCTION enforce_collection_group_role_grant_visibility();

CREATE OR REPLACE FUNCTION enforce_collection_visibility_grant_invariant()
RETURNS trigger AS $$
BEGIN
    PERFORM 1
    FROM collections
    WHERE org_id = NEW.org_id AND id = NEW.id
    FOR NO KEY UPDATE;

    IF NEW.visibility <> 'groups' THEN
        IF EXISTS (
            SELECT 1
            FROM collection_group_access g
            WHERE g.org_id = NEW.org_id AND g.collection_id = NEW.id
        ) OR EXISTS (
            SELECT 1
            FROM collection_role_access r
            WHERE r.org_id = NEW.org_id AND r.collection_id = NEW.id
        ) THEN
            RAISE EXCEPTION 'cannot change visibility away from groups while group/role grants remain';
        END IF;
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS collections_enforce_visibility_grant_invariant ON collections;
CREATE TRIGGER collections_enforce_visibility_grant_invariant
    BEFORE UPDATE OF visibility ON collections
    FOR EACH ROW EXECUTE FUNCTION enforce_collection_visibility_grant_invariant();

-- Replace the 0033 helper so cross-org UPDATE bumps both orgs exactly once;
-- same-org INSERT/UPDATE/DELETE still bump once.
CREATE OR REPLACE FUNCTION bump_org_acl_version() RETURNS trigger AS $$
DECLARE
    target_org uuid;
BEGIN
    IF TG_OP = 'UPDATE' AND OLD.org_id IS DISTINCT FROM NEW.org_id THEN
        IF OLD.org_id IS NOT NULL THEN
            UPDATE orgs SET acl_version = acl_version + 1 WHERE id = OLD.org_id;
        END IF;
        IF NEW.org_id IS NOT NULL THEN
            UPDATE orgs SET acl_version = acl_version + 1 WHERE id = NEW.org_id;
        END IF;
    ELSE
        target_org := COALESCE(NEW.org_id, OLD.org_id);
        IF target_org IS NOT NULL THEN
            UPDATE orgs SET acl_version = acl_version + 1 WHERE id = target_org;
        END IF;
    END IF;
    RETURN COALESCE(NEW, OLD);
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS collection_group_access_bump_acl_version ON collection_group_access;
CREATE TRIGGER collection_group_access_bump_acl_version
    AFTER INSERT OR UPDATE OR DELETE ON collection_group_access
    FOR EACH ROW EXECUTE FUNCTION bump_org_acl_version();

DROP TRIGGER IF EXISTS collection_role_access_bump_acl_version ON collection_role_access;
CREATE TRIGGER collection_role_access_bump_acl_version
    AFTER INSERT OR UPDATE OR DELETE ON collection_role_access
    FOR EACH ROW EXECUTE FUNCTION bump_org_acl_version();

DROP TRIGGER IF EXISTS group_memberships_bump_acl_version ON group_memberships;
CREATE TRIGGER group_memberships_bump_acl_version
    AFTER INSERT OR UPDATE OR DELETE ON group_memberships
    FOR EACH ROW EXECUTE FUNCTION bump_org_acl_version();
