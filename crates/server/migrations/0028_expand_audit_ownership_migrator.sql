-- Phase: 1B
-- Owner: operations-owner, security-owner
-- Change: expand
-- Lock/data risk: ownership REASSIGN + grant revoke; no row rewrites.
-- Rollback compatibility: ownership can be reassigned back by superuser.
-- Migrator (CURRENT_USER) must own audit_log + every audit trigger function
-- (enforce_immutability + validate_insert). Fail closed if markhand_app owns
-- any of them. Exact app grants: SELECT+INSERT only on audit_log.

DO $$
DECLARE
    tbl_owner text;
    fn_owner text;
    fn_name text;
    required_fns text[] := ARRAY[
        'audit_log_enforce_immutability',
        'audit_log_validate_insert'
    ];
BEGIN
    SELECT pg_get_userbyid(c.relowner) INTO tbl_owner
    FROM pg_class c
    JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE n.nspname = 'public' AND c.relname = 'audit_log' AND c.relkind = 'r';

    IF tbl_owner IS NULL THEN
        RAISE EXCEPTION 'audit_log table missing'
            USING ERRCODE = 'undefined_table';
    END IF;

    IF tbl_owner = 'markhand_app' THEN
        -- Requires: GRANT markhand_app TO migrator WITH INHERIT TRUE (PG16+).
        EXECUTE 'ALTER TABLE audit_log OWNER TO CURRENT_USER';
    ELSIF tbl_owner <> session_user AND tbl_owner <> current_user THEN
        -- Take ownership when bootstrap/superuser left objects owned by another
        -- non-app role only if we already own via SET ROLE; otherwise leave and
        -- verify below that app does not own.
        NULL;
    END IF;

    -- Transfer app-owned trigger/stats functions; then verify required ones.
    FOR fn_name IN
        SELECT p.proname
        FROM pg_proc p
        JOIN pg_namespace n ON n.oid = p.pronamespace
        WHERE n.nspname = 'public'
          AND p.proname = ANY (required_fns || ARRAY['markhand_job_queue_stats'])
          AND pg_get_userbyid(p.proowner) = 'markhand_app'
    LOOP
        EXECUTE format('ALTER FUNCTION %I() OWNER TO CURRENT_USER', fn_name);
    END LOOP;

    -- Fail if app still owns audit_log.
    SELECT pg_get_userbyid(c.relowner) INTO tbl_owner
    FROM pg_class c
    JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE n.nspname = 'public' AND c.relname = 'audit_log' AND c.relkind = 'r';
    IF tbl_owner = 'markhand_app' THEN
        RAISE EXCEPTION 'audit_log still owned by markhand_app after migrator transfer'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    -- Fail if app owns any required audit trigger function (including validate_insert).
    FOREACH fn_name IN ARRAY required_fns LOOP
        SELECT pg_get_userbyid(p.proowner) INTO fn_owner
        FROM pg_proc p
        JOIN pg_namespace n ON n.oid = p.pronamespace
        WHERE n.nspname = 'public' AND p.proname = fn_name
        LIMIT 1;
        IF fn_owner IS NULL THEN
            RAISE EXCEPTION 'required audit function % missing', fn_name
                USING ERRCODE = 'undefined_function';
        END IF;
        IF fn_owner = 'markhand_app' THEN
            RAISE EXCEPTION '% still owned by markhand_app', fn_name
                USING ERRCODE = 'insufficient_privilege';
        END IF;
    END LOOP;
END
$$;

-- Exact app grants: DML insert/select only; never schema CREATE/ALTER/mutate.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'markhand_app') THEN
        REVOKE CREATE ON SCHEMA public FROM markhand_app;
        GRANT USAGE ON SCHEMA public TO markhand_app;

        REVOKE ALL ON TABLE audit_log FROM markhand_app;
        GRANT SELECT, INSERT ON TABLE audit_log TO markhand_app;
        REVOKE UPDATE, DELETE, TRUNCATE, REFERENCES, TRIGGER ON TABLE audit_log FROM markhand_app;

        REVOKE ALL ON FUNCTION audit_log_enforce_immutability() FROM markhand_app;
        REVOKE ALL ON FUNCTION audit_log_validate_insert() FROM markhand_app;
        GRANT EXECUTE ON FUNCTION markhand_job_queue_stats() TO markhand_app;
    END IF;
END
$$;
