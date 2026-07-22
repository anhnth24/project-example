-- P1B-O03: readiness may certify only after verified zero-drift convergence.
-- Extends 0022 helpers; does not rewrite historical migration files.

ALTER TABLE runtime_readiness
    ADD COLUMN IF NOT EXISTS zero_drift_certified boolean NOT NULL DEFAULT false,
    ADD COLUMN IF NOT EXISTS last_reconcile_result text,
    ADD COLUMN IF NOT EXISTS last_drift_total bigint NOT NULL DEFAULT -1,
    ADD COLUMN IF NOT EXISTS generation_reconcile_completed bigint NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS generation_reconcile_drift bigint NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS generation_reconcile_error bigint NOT NULL DEFAULT 0;

-- Clear zero-drift markers whenever a generation is opened.
CREATE OR REPLACE FUNCTION markhand_runtime_readiness_open(
    p_key text,
    p_detail text
)
RETURNS bigint
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public
AS $$
DECLARE
    new_generation bigint;
BEGIN
    UPDATE runtime_readiness
       SET ready = false,
           zero_drift_certified = false,
           last_reconcile_result = NULL,
           last_drift_total = -1,
           generation_reconcile_completed = 0,
           generation_reconcile_drift = 0,
           generation_reconcile_error = 0,
           generation = generation + 1,
           updated_at = now(),
           detail = p_detail
     WHERE key = p_key
     RETURNING generation INTO new_generation;

    IF new_generation IS NULL THEN
        INSERT INTO runtime_readiness (
            key, ready, generation, certified_generation, detail,
            zero_drift_certified, last_drift_total
        )
        VALUES (p_key, false, 1, 0, p_detail, false, -1)
        RETURNING generation INTO new_generation;
    END IF;

    RETURN new_generation;
END;
$$;

-- Record one finished reconcile job outcome for the current generation.
-- drift_total > 0 or result=error prevents later try_ready certification.
CREATE OR REPLACE FUNCTION markhand_runtime_readiness_record_reconcile(
    p_key text,
    p_result text,
    p_drift_total bigint,
    p_detail text
)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public
AS $$
DECLARE
    current_generation bigint;
    normalized text;
BEGIN
    IF p_drift_total IS NULL OR p_drift_total < 0 THEN
        RAISE EXCEPTION 'drift_total must be >= 0';
    END IF;

    normalized := lower(coalesce(p_result, ''));
    IF normalized NOT IN ('success', 'drift', 'error') THEN
        RAISE EXCEPTION 'reconcile result must be success|drift|error';
    END IF;

    SELECT generation INTO current_generation
      FROM runtime_readiness
     WHERE key = p_key
     FOR UPDATE;

    IF current_generation IS NULL OR current_generation = 0 THEN
        RETURN false;
    END IF;

    UPDATE runtime_readiness
       SET last_reconcile_result = normalized,
           last_drift_total = p_drift_total,
           generation_reconcile_completed = generation_reconcile_completed + 1,
           generation_reconcile_drift = generation_reconcile_drift
               + CASE WHEN normalized = 'drift' OR p_drift_total > 0 THEN 1 ELSE 0 END,
           generation_reconcile_error = generation_reconcile_error
               + CASE WHEN normalized = 'error' THEN 1 ELSE 0 END,
           zero_drift_certified = false,
           ready = false,
           updated_at = now(),
           detail = coalesce(p_detail, normalized)
     WHERE key = p_key;

    RETURN false;
END;
$$;

-- Certify ready only when queue idle AND this generation has zero drift/error
-- and either completed at least one reconcile or an explicit empty-catalog success.
CREATE OR REPLACE FUNCTION markhand_runtime_readiness_try_ready(
    p_key text,
    p_detail text
)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public
AS $$
DECLARE
    current_generation bigint;
    pending bigint;
    completed bigint;
    drift_jobs bigint;
    error_jobs bigint;
    last_result text;
    last_drift bigint;
    is_ready boolean;
BEGIN
    SELECT generation,
           generation_reconcile_completed,
           generation_reconcile_drift,
           generation_reconcile_error,
           last_reconcile_result,
           last_drift_total
      INTO current_generation, completed, drift_jobs, error_jobs, last_result, last_drift
      FROM runtime_readiness
     WHERE key = p_key
     FOR UPDATE;

    IF current_generation IS NULL THEN
        RETURN false;
    END IF;

    IF current_generation = 0 THEN
        UPDATE runtime_readiness
           SET ready = false,
               zero_drift_certified = false,
               updated_at = now(),
               detail = 'awaiting startup bootstrap'
         WHERE key = p_key;
        RETURN false;
    END IF;

    pending := markhand_pending_reconcile_jobs();
    IF pending > 0 THEN
        UPDATE runtime_readiness
           SET ready = false,
               zero_drift_certified = false,
               updated_at = now(),
               detail = coalesce(p_detail, 'pending reconcile jobs')
         WHERE key = p_key;
        RETURN false;
    END IF;

    IF drift_jobs > 0 OR error_jobs > 0 OR coalesce(last_drift, -1) > 0 THEN
        UPDATE runtime_readiness
           SET ready = false,
               zero_drift_certified = false,
               updated_at = now(),
               detail = coalesce(p_detail, 'drift or error blocks readiness')
         WHERE key = p_key;
        RETURN false;
    END IF;

    -- Require explicit success evidence for this generation (completed>0 with
    -- last_result=success and last_drift=0). Empty-catalog bootstrap records
    -- a synthetic success with completed=1 via record_reconcile.
    IF completed <= 0 OR last_result IS DISTINCT FROM 'success' OR last_drift IS DISTINCT FROM 0 THEN
        UPDATE runtime_readiness
           SET ready = false,
               zero_drift_certified = false,
               updated_at = now(),
               detail = coalesce(p_detail, 'zero-drift convergence not verified')
         WHERE key = p_key;
        RETURN false;
    END IF;

    UPDATE runtime_readiness
       SET ready = true,
           zero_drift_certified = true,
           certified_generation = current_generation,
           updated_at = now(),
           detail = coalesce(p_detail, 'zero-drift reconcile generation certified')
     WHERE key = p_key
     RETURNING ready INTO is_ready;

    RETURN coalesce(is_ready, false);
END;
$$;

-- Global document count (bypasses RLS) for empty-catalog bootstrap.
CREATE OR REPLACE FUNCTION markhand_document_count()
RETURNS bigint
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = public
AS $$
    SELECT count(*)::bigint FROM documents;
$$;

REVOKE ALL ON FUNCTION markhand_runtime_readiness_record_reconcile(text, text, bigint, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION markhand_document_count() FROM PUBLIC;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'markhand_app') THEN
        GRANT EXECUTE ON FUNCTION markhand_runtime_readiness_record_reconcile(text, text, bigint, text) TO markhand_app;
        GRANT EXECUTE ON FUNCTION markhand_document_count() TO markhand_app;
        GRANT EXECUTE ON FUNCTION markhand_runtime_readiness_open(text, text) TO markhand_app;
        GRANT EXECUTE ON FUNCTION markhand_runtime_readiness_try_ready(text, text) TO markhand_app;
    END IF;
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'markhand_test') THEN
        GRANT EXECUTE ON FUNCTION markhand_runtime_readiness_record_reconcile(text, text, bigint, text) TO markhand_test;
        GRANT EXECUTE ON FUNCTION markhand_document_count() TO markhand_test;
        GRANT EXECUTE ON FUNCTION markhand_runtime_readiness_open(text, text) TO markhand_test;
        GRANT EXECUTE ON FUNCTION markhand_runtime_readiness_try_ready(text, text) TO markhand_test;
    END IF;
END
$$;
