-- Phase: 1B
-- Owner: operations-owner, security-owner
-- Change: expand
-- Lock/data risk: additive triggers/functions on audit_log; no rewrite of existing rows.
-- Rollback compatibility: drop triggers/functions only; table data retained.
-- Enforce append-only audit_log and refuse secret-looking metadata keys.

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
-- Does not restrict action/resource enums — application allowlists evolve faster.
CREATE OR REPLACE FUNCTION audit_log_validate_insert()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    key text;
BEGIN
    IF NEW.outcome NOT IN ('success', 'deny', 'error', 'intent') THEN
        RAISE EXCEPTION 'audit_log outcome invalid'
            USING ERRCODE = 'check_violation';
    END IF;
    IF NEW.request_id IS NULL
        OR NEW.request_id !~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
    THEN
        RAISE EXCEPTION 'audit_log request_id must be a UUID'
            USING ERRCODE = 'check_violation';
    END IF;
    IF jsonb_typeof(NEW.metadata) <> 'object' THEN
        RAISE EXCEPTION 'audit_log metadata must be a JSON object'
            USING ERRCODE = 'check_violation';
    END IF;
    FOR key IN SELECT * FROM jsonb_object_keys(NEW.metadata)
    LOOP
        IF lower(key) IN (
            'password', 'secret', 'authorization', 'cookie', 'prompt', 'answer',
            'question', 'email', 'api_key', 'apikey', 'object_key', 'object_keys',
            'document_content', 'markdown', 'signed_url', 'access_token',
            'refresh_token', 'token', 'database_url'
        ) THEN
            RAISE EXCEPTION 'audit_log metadata key forbidden'
                USING ERRCODE = 'check_violation';
        END IF;
    END LOOP;
    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS trg_audit_log__validate_insert ON audit_log;
CREATE TRIGGER trg_audit_log__validate_insert
    BEFORE INSERT ON audit_log
    FOR EACH ROW
    EXECUTE FUNCTION audit_log_validate_insert();
