-- Phase: 1B
-- Owner: operations-owner, security-owner
-- Change: expand
-- Lock/data risk: role/grant adjustments + SECURITY DEFINER stats function; no row rewrites.
-- Rollback compatibility: drop function; restore prior grants manually if needed.
-- Separates migrator ownership from markhand_app runtime grants (O01 / Sol #5).

-- Authoritative queue stats for metrics (bypasses FORCE RLS via SECURITY DEFINER).
CREATE OR REPLACE FUNCTION markhand_job_queue_stats()
RETURNS TABLE(job_type text, depth bigint, age_seconds double precision)
LANGUAGE sql
SECURITY DEFINER
SET search_path = public
AS $$
    SELECT j.job_type::text,
           COUNT(*)::bigint,
           COALESCE(EXTRACT(EPOCH FROM (now() - MIN(j.created_at)))::float8, 0)
    FROM jobs j
    WHERE j.status = 'pending'
    GROUP BY j.job_type;
$$;

REVOKE ALL ON FUNCTION markhand_job_queue_stats() FROM PUBLIC;

-- Runtime app role: DML only, never schema ownership / CREATE.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'markhand_app') THEN
        GRANT USAGE ON SCHEMA public TO markhand_app;
        -- Prefer least privilege: revoke broad CREATE if previously granted.
        BEGIN
            EXECUTE 'REVOKE CREATE ON SCHEMA public FROM markhand_app';
        EXCEPTION WHEN insufficient_privilege OR undefined_object THEN
            NULL;
        END;

        GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO markhand_app;
        GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO markhand_app;
        GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA public TO markhand_app;

        -- Append-only audit: app may INSERT (+ SELECT for its own reads), never mutate.
        REVOKE UPDATE, DELETE, TRUNCATE ON TABLE audit_log FROM markhand_app;
        GRANT SELECT, INSERT ON TABLE audit_log TO markhand_app;

        -- Cannot disable/drop immutability triggers without ownership.
        -- Ensure trigger functions are not owned by markhand_app when possible.
        BEGIN
            EXECUTE 'ALTER FUNCTION audit_log_enforce_immutability() OWNER TO CURRENT_USER';
            EXECUTE 'ALTER FUNCTION audit_log_validate_insert() OWNER TO CURRENT_USER';
            EXECUTE 'ALTER FUNCTION markhand_job_queue_stats() OWNER TO CURRENT_USER';
        EXCEPTION WHEN insufficient_privilege OR undefined_function THEN
            NULL;
        END;

        GRANT EXECUTE ON FUNCTION markhand_job_queue_stats() TO markhand_app;
    END IF;
END
$$;
