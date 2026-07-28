-- Phase: 1C
-- Owner: security-owner, storage-owner
-- Change: expand
-- Lock/data risk: two brand-new small global tables + a provisioning function;
--   no ALTER/rewrite of any existing table, no new RLS policy, no data touched
--   on the existing per-org `roles`/`role_permissions` rows (including POC's).
-- Rollback compatibility: additive only; dropping the two new tables/function
--   in a later contract migration would not affect any existing org's
--   `roles`/`role_permissions` rows, which remain the actual authorization
--   source (see design note below).
--
-- 1C-03/1C-01: canonical, org-independent RBAC catalog.
--
-- `roles`/`role_permissions` (migrations/0003) are per-org tables today: every
-- org needs its own copy of the four system roles + the full permission
-- matrix, and until now the ONLY place that ever got seeded was the POC org
-- (migrations/0011/0017/0019), hardcoded by UUID. That is exactly what blocks
-- `POST /orgs` (1C-01): a freshly created org would have zero roles and every
-- permission check would fail closed forever.
--
-- This migration introduces `role_catalog` + `role_catalog_permissions` as the
-- one org-independent source of truth for "what do the four system roles
-- grant" and a `provision_org_role_catalog(org_id)` function that copies that
-- catalog into a *specific* org's `roles`/`role_permissions` rows. The new
-- `services::orgs::create_org` (1C-01) calls this function once, in the same
-- transaction as the org/membership insert — so an operator creating an org
-- never has to hand-write a per-org seed migration again.
--
-- Deliberately NOT an `AFTER INSERT ON orgs` trigger: several existing test
-- fixtures (`tests/common::seed_user_with_permissions`, `db::orgs::ensure_exists`)
-- insert directly into `orgs` to build deny/permission-less fixtures (an
-- "owner" membership that intentionally holds zero permissions, to test
-- fail-closed paths). A blanket trigger firing on every `orgs` insert would
-- silently grant those fixtures the full canonical permission set and flip a
-- large number of existing deny tests to allow. Explicit, opt-in provisioning
-- (a function the real creation path calls) gets the same "no manual seed"
-- outcome for real org creation without that blast radius.
--
-- Also deliberately NOT a rewrite of `auth/permissions.rs`'s (and
-- `services/upload/saga.rs`'s, `db/search.rs`'s, `services/acl_mutate.rs`'s)
-- existing `roles`/`role_permissions` joins: `acl_mutate::
-- revoke_role_permission_for_principal` intentionally DELETEs a per-org
-- `role_permissions` row for incident containment (exercised by
-- `tests/uploads.rs`/`tests/sse_stream_readiness.rs`'s in-flight ACL-revoke
-- tests) — per-org rows must stay the live, mutable authorization state.
-- `role_catalog`/`role_catalog_permissions` is the immutable canonical
-- *template* those per-org rows are provisioned from, not a second place the
-- resolver reads from at request time.

CREATE TABLE role_catalog (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    code text NOT NULL,
    name text NOT NULL CHECK (length(trim(name)) > 0),
    is_system boolean NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_role_catalog__code UNIQUE (code),
    CONSTRAINT ck_role_catalog__code CHECK (code ~ '^[a-z][a-z0-9_]{1,31}$')
);

CREATE TABLE role_catalog_permissions (
    role_code text NOT NULL REFERENCES role_catalog (code) ON DELETE RESTRICT,
    permission_id uuid NOT NULL REFERENCES permissions (id) ON DELETE RESTRICT,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (role_code, permission_id)
);

CREATE INDEX idx_role_catalog_permissions__permission_id
    ON role_catalog_permissions (permission_id);

-- ---------------------------------------------------------------------
-- Idempotent canonical seed. Mirrors the POC org's CURRENT effective matrix
-- exactly (migrations/0011 + 0017 + 0019) so globalizing the catalog changes
-- no existing behavior: owner/admin share the same 8 permissions POC's
-- owner/admin roles hold today; editor/viewer keep their narrower subsets.
-- `doc.quarantine.review` (migrations/0023) is intentionally granted to no
-- role here either — it isn't granted to any POC role today, and this
-- migration's job is to globalize the existing catalog, not to change it.
-- ---------------------------------------------------------------------

INSERT INTO role_catalog (code, name, is_system)
VALUES
    ('owner', 'Owner', true),
    ('admin', 'Admin', true),
    ('editor', 'Editor', true),
    ('viewer', 'Viewer', true)
ON CONFLICT (code) DO NOTHING;

INSERT INTO role_catalog_permissions (role_code, permission_id)
SELECT r.code, p.id
FROM (VALUES ('owner'), ('admin')) AS r (code)
CROSS JOIN permissions p
WHERE p.code IN (
    'doc.upload', 'doc.delete', 'doc.publish', 'qa.query',
    'member.manage', 'audit.view', 'qa.history', 'jobs.system'
)
ON CONFLICT (role_code, permission_id) DO NOTHING;

INSERT INTO role_catalog_permissions (role_code, permission_id)
SELECT 'editor', p.id FROM permissions p
WHERE p.code IN ('doc.upload', 'doc.publish', 'qa.query')
ON CONFLICT (role_code, permission_id) DO NOTHING;

INSERT INTO role_catalog_permissions (role_code, permission_id)
SELECT 'viewer', p.id FROM permissions p
WHERE p.code = 'qa.query'
ON CONFLICT (role_code, permission_id) DO NOTHING;

-- ---------------------------------------------------------------------
-- Immutability: every row in this catalog is a system role/grant today (no
-- custom role builder — out of scope per 1C-03). Block UPDATE/DELETE
-- unconditionally, same append-only-by-trigger shape as `audit_log`
-- (migrations/0026). The migrator (table owner) can still intervene via
-- `ALTER TABLE ... DISABLE TRIGGER ALL` in a dedicated, reviewed migration —
-- deliberately not something `markhand_app` can do (no ALTER/ownership).
-- ---------------------------------------------------------------------

CREATE OR REPLACE FUNCTION role_catalog_enforce_immutability()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION '% is immutable (global RBAC system-role catalog): % is forbidden', TG_TABLE_NAME, TG_OP
        USING ERRCODE = 'integrity_constraint_violation';
END;
$$;

DROP TRIGGER IF EXISTS trg_role_catalog__immutability ON role_catalog;
CREATE TRIGGER trg_role_catalog__immutability
    BEFORE UPDATE OR DELETE ON role_catalog
    FOR EACH ROW
    EXECUTE FUNCTION role_catalog_enforce_immutability();

DROP TRIGGER IF EXISTS trg_role_catalog__immutability_truncate ON role_catalog;
CREATE TRIGGER trg_role_catalog__immutability_truncate
    BEFORE TRUNCATE ON role_catalog
    FOR EACH STATEMENT
    EXECUTE FUNCTION role_catalog_enforce_immutability();

DROP TRIGGER IF EXISTS trg_role_catalog_permissions__immutability ON role_catalog_permissions;
CREATE TRIGGER trg_role_catalog_permissions__immutability
    BEFORE UPDATE OR DELETE ON role_catalog_permissions
    FOR EACH ROW
    EXECUTE FUNCTION role_catalog_enforce_immutability();

DROP TRIGGER IF EXISTS trg_role_catalog_permissions__immutability_truncate ON role_catalog_permissions;
CREATE TRIGGER trg_role_catalog_permissions__immutability_truncate
    BEFORE TRUNCATE ON role_catalog_permissions
    FOR EACH STATEMENT
    EXECUTE FUNCTION role_catalog_enforce_immutability();

-- ---------------------------------------------------------------------
-- Provisioning: copy the canonical catalog into one org's `roles`/
-- `role_permissions` rows. Idempotent (ON CONFLICT DO NOTHING both steps) —
-- safe to call more than once for the same org (e.g. a retried create, or a
-- future manual backfill of a pre-existing org). `roles`/`role_permissions`
-- are FORCE RLS'd by `org_id` (migrations/0010), so this sets the
-- transaction-local GUC itself (same `set_config(..., true)` shape
-- migrations/0019's backfill loop already uses) rather than requiring the
-- caller to have done so first.
-- ---------------------------------------------------------------------

CREATE OR REPLACE FUNCTION provision_org_role_catalog(p_org_id uuid)
RETURNS void
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM set_config('app.org_id', p_org_id::text, true);

    INSERT INTO roles (id, org_id, code, name, is_system)
    SELECT gen_random_uuid(), p_org_id, rc.code, rc.name, rc.is_system
    FROM role_catalog rc
    ON CONFLICT (org_id, code) DO NOTHING;

    INSERT INTO role_permissions (org_id, role_id, permission_id)
    SELECT p_org_id, r.id, rcp.permission_id
    FROM role_catalog_permissions rcp
    JOIN roles r ON r.org_id = p_org_id AND r.code = rcp.role_code
    ON CONFLICT (role_id, permission_id) DO NOTHING;
END;
$$;

REVOKE ALL ON FUNCTION provision_org_role_catalog(uuid) FROM PUBLIC;

-- Least privilege: markhand_app may read the catalog and call the
-- provisioning function, never write the catalog directly.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'markhand_app') THEN
        GRANT SELECT ON role_catalog, role_catalog_permissions TO markhand_app;
        REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON role_catalog, role_catalog_permissions FROM markhand_app;
        GRANT EXECUTE ON FUNCTION provision_org_role_catalog(uuid) TO markhand_app;
    END IF;
END
$$;
