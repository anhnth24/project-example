-- Phase: 1B
-- Owner: ops-owner, server-owner
-- Change: expand
-- Lock/data risk: additive global fence table + permission; no existing row rewrite.
-- Rollback compatibility: drop ops_fences; revoke jobs.system grants.
-- Durable restore/reconcile fence (not seed-org hardcode) + documentless job permission.

CREATE TABLE IF NOT EXISTS ops_fences (
    name text PRIMARY KEY,
    reason text NOT NULL,
    active boolean NOT NULL DEFAULT true,
    set_at timestamptz NOT NULL DEFAULT now(),
    cleared_at timestamptz,
    set_by text,
    attestation_sha256 text,
    CONSTRAINT ck_ops_fences__clear_state CHECK (
        (active = true AND cleared_at IS NULL)
        OR (active = false AND cleared_at IS NOT NULL)
    )
);

CREATE INDEX IF NOT EXISTS idx_ops_fences__active
    ON ops_fences (active)
    WHERE active;

INSERT INTO permissions (id, code, description)
VALUES (
    '33333333-3333-3333-3333-333333333308',
    'jobs.system',
    'Enqueue and observe system jobs without a document scope'
)
ON CONFLICT (id) DO NOTHING;

-- Grant jobs.system to owner/admin roles in every org (no seed-org hardcode).
DO $$
DECLARE
    org_row record;
BEGIN
    FOR org_row IN
        SELECT DISTINCT org_id FROM roles WHERE code IN ('owner', 'admin')
    LOOP
        PERFORM set_config('app.org_id', org_row.org_id::text, true);
        INSERT INTO role_permissions (org_id, role_id, permission_id)
        SELECT roles.org_id, roles.id, '33333333-3333-3333-3333-333333333308'::uuid
        FROM roles
        WHERE roles.org_id = org_row.org_id
          AND roles.code IN ('owner', 'admin')
        ON CONFLICT (role_id, permission_id) DO NOTHING;
    END LOOP;
END $$;
