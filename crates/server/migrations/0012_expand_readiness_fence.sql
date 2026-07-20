-- Phase: 1B
-- Owner: storage-owner, security-owner
-- Change: expand
-- Lock/data risk: creates one system singleton table and seeds one row; no tenant-table locks.
-- Rollback compatibility: additive readiness fence; older application versions ignore it.
-- System restore/reconcile fence. This table intentionally has no tenant data and no RLS.

CREATE TABLE readiness_fence (
    id smallint PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    state text NOT NULL CHECK (state IN ('ready', 'reconciling', 'restoring')) DEFAULT 'ready',
    reason text,
    updated_at timestamptz NOT NULL DEFAULT now()
);

INSERT INTO readiness_fence (id, state, reason)
VALUES (1, 'ready', NULL)
ON CONFLICT (id) DO NOTHING;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'markhand_app') THEN
        GRANT SELECT, UPDATE ON TABLE readiness_fence TO markhand_app;
    END IF;
END
$$;
