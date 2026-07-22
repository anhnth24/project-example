-- Phase: 1B
-- Owner: operations-owner, security-owner
-- Change: expand
-- Lock/data risk: additive triggers/functions on audit_log; no rewrite of existing rows.
-- Rollback compatibility: drop triggers/functions only; table data retained.
-- Enforce append-only audit_log and tighten tenant isolation for writes.

CREATE OR REPLACE FUNCTION audit_log_enforce_immutability()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION 'audit_log is append-only: % is forbidden', TG_OP
        USING ERRCODE = 'integrity_constraint_violation';
END;
$$;

DROP TRIGGER IF EXISTS trg_audit_log__immutability ON audit_log;
CREATE TRIGGER trg_audit_log__immutability
    BEFORE UPDATE OR DELETE ON audit_log
    FOR EACH ROW
    EXECUTE FUNCTION audit_log_enforce_immutability();

-- Statement-level truncate protection (row triggers do not fire for TRUNCATE).
DROP TRIGGER IF EXISTS trg_audit_log__immutability_truncate ON audit_log;
CREATE TRIGGER trg_audit_log__immutability_truncate
    BEFORE TRUNCATE ON audit_log
    FOR EACH STATEMENT
    EXECUTE FUNCTION audit_log_enforce_immutability();

-- Refuse secret-looking metadata keys at the database boundary (defense in depth).
CREATE OR REPLACE FUNCTION audit_log_validate_insert()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    key text;
    val jsonb;
BEGIN
    IF NEW.action NOT IN (
        'auth.login',
        'auth.deny',
        'auth.logout',
        'auth.refresh',
        'auth.refresh.reuse',
        'auth.revoke_all',
        'document.upload',
        'document.delete',
        'document.tombstone',
        'document.publish',
        'document.reindex',
        'document.purge',
        'document.purge_objects',
        'job.enqueue',
        'quota.deny',
        'reconcile.repair',
        'vector.cleanup_intent',
        'object.cleanup'
    ) THEN
        RAISE EXCEPTION 'audit_log action invalid'
            USING ERRCODE = 'check_violation';
    END IF;
    IF NEW.resource_type NOT IN (
        'session',
        'document',
        'job',
        'quota',
        'object'
    ) THEN
        RAISE EXCEPTION 'audit_log resource_type invalid'
            USING ERRCODE = 'check_violation';
    END IF;
    IF NEW.outcome NOT IN ('success', 'deny', 'error') THEN
        RAISE EXCEPTION 'audit_log outcome invalid'
            USING ERRCODE = 'check_violation';
    END IF;
    IF NEW.request_id IS NULL
        OR NEW.request_id !~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
    THEN
        RAISE EXCEPTION 'audit_log request_id must be a UUID'
            USING ERRCODE = 'check_violation';
    END IF;
    IF NEW.resource_id IS NOT NULL AND (
        NEW.resource_id !~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
        OR position('mh1.' in NEW.resource_id) > 0
        OR position('Bearer ' in NEW.resource_id) > 0
        OR NEW.resource_id LIKE 'eyJ%'
    ) THEN
        RAISE EXCEPTION 'audit_log resource_id invalid'
            USING ERRCODE = 'check_violation';
    END IF;
    IF jsonb_typeof(NEW.metadata) <> 'object' THEN
        RAISE EXCEPTION 'audit_log metadata must be a JSON object'
            USING ERRCODE = 'check_violation';
    END IF;
    FOR key, val IN SELECT * FROM jsonb_each(NEW.metadata)
    LOOP
        IF lower(key) IN (
            'password', 'secret', 'authorization', 'cookie', 'prompt', 'answer',
            'question', 'email', 'api_key', 'apikey', 'object_key', 'object_keys',
            'document_content', 'signed_url', 'refresh_token', 'access_token',
            'capability', 'raw_body', 'body', 'text', 'markdown'
        ) THEN
            RAISE EXCEPTION 'audit_log metadata key is forbidden: %', key
                USING ERRCODE = 'check_violation';
        END IF;
        IF jsonb_typeof(val) IN ('object', 'array') THEN
            RAISE EXCEPTION 'audit_log metadata value must be scalar'
                USING ERRCODE = 'check_violation';
        END IF;
    END LOOP;
    IF NEW.metadata::text ~* '(CANARY_SECRET_TOKEN|CANARY_DOCUMENT_TEXT|CANARY_PROMPT_TEXT|CANARY_ANSWER_TEXT|CANARY_API_KEY|"mh1\.|"Bearer )' THEN
        RAISE EXCEPTION 'audit_log metadata contains forbidden material'
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS trg_audit_log__validate_insert ON audit_log;
CREATE TRIGGER trg_audit_log__validate_insert
    BEFORE INSERT ON audit_log
    FOR EACH ROW
    EXECUTE FUNCTION audit_log_validate_insert();

-- Ensure RLS remains forced (fresh environments / repair).
ALTER TABLE audit_log ENABLE ROW LEVEL SECURITY;
ALTER TABLE audit_log FORCE ROW LEVEL SECURITY;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_policies
        WHERE schemaname = 'public'
          AND tablename = 'audit_log'
          AND policyname = 'audit_log_org_isolation'
    ) THEN
        CREATE POLICY audit_log_org_isolation ON audit_log
            USING (org_id = markhand_current_org_id())
            WITH CHECK (org_id = markhand_current_org_id());
    END IF;
END $$;

-- Runtime roles may INSERT/SELECT only; never UPDATE/DELETE/TRUNCATE.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'markhand_app') THEN
        REVOKE UPDATE, DELETE, TRUNCATE ON TABLE audit_log FROM markhand_app;
        GRANT SELECT, INSERT ON TABLE audit_log TO markhand_app;
    END IF;
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'markhand_test') THEN
        REVOKE UPDATE, DELETE, TRUNCATE ON TABLE audit_log FROM markhand_test;
        GRANT SELECT, INSERT ON TABLE audit_log TO markhand_test;
    END IF;
END $$;

REVOKE ALL ON FUNCTION audit_log_enforce_immutability() FROM PUBLIC;
REVOKE ALL ON FUNCTION audit_log_validate_insert() FROM PUBLIC;
