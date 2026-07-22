-- P1B-R06: durable generation-aware startup reconciliation / readiness fence.
-- Default is not ready until startup bootstrap records a generation and that
-- generation is certified with no pending/leased reconcile jobs.
--
-- Global pending-job counts must use SECURITY DEFINER helpers: jobs are RLS
-- forced, so a raw SELECT without org context would under-count.

CREATE TABLE runtime_readiness (
    key text PRIMARY KEY,
    ready boolean NOT NULL DEFAULT false,
    generation bigint NOT NULL DEFAULT 0,
    certified_generation bigint NOT NULL DEFAULT 0,
    updated_at timestamptz NOT NULL DEFAULT now(),
    detail text
);

INSERT INTO runtime_readiness (key, ready, generation, certified_generation, detail)
VALUES ('startup_reconciliation', false, 0, 0, 'awaiting startup bootstrap');

-- Cross-tenant pending/leased reconcile count (bypasses RLS via SECURITY DEFINER).
CREATE OR REPLACE FUNCTION markhand_pending_reconcile_jobs()
RETURNS bigint
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = public
AS $$
    SELECT count(*)::bigint
    FROM jobs
    WHERE job_type = 'reconcile'
      AND status IN ('queued', 'leased');
$$;

-- Atomically close readiness and advance the generation (enqueue / bootstrap).
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
           generation = generation + 1,
           updated_at = now(),
           detail = p_detail
     WHERE key = p_key
     RETURNING generation INTO new_generation;

    IF new_generation IS NULL THEN
        INSERT INTO runtime_readiness (key, ready, generation, certified_generation, detail)
        VALUES (p_key, false, 1, 0, p_detail)
        RETURNING generation INTO new_generation;
    END IF;

    RETURN new_generation;
END;
$$;

-- Atomically certify ready for the current generation when the queue is idle.
-- Returns true when the marker is ready after the call.
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
    is_ready boolean;
BEGIN
    SELECT generation INTO current_generation
      FROM runtime_readiness
     WHERE key = p_key
     FOR UPDATE;

    IF current_generation IS NULL THEN
        RETURN false;
    END IF;

    -- Never certify a generation that was never opened (bootstrap required).
    IF current_generation = 0 THEN
        UPDATE runtime_readiness
           SET ready = false,
               updated_at = now(),
               detail = 'awaiting startup bootstrap'
         WHERE key = p_key;
        RETURN false;
    END IF;

    pending := markhand_pending_reconcile_jobs();
    IF pending > 0 THEN
        UPDATE runtime_readiness
           SET ready = false,
               updated_at = now(),
               detail = coalesce(p_detail, 'pending reconcile jobs')
         WHERE key = p_key;
        RETURN false;
    END IF;

    UPDATE runtime_readiness
       SET ready = true,
           certified_generation = current_generation,
           updated_at = now(),
           detail = coalesce(p_detail, 'reconcile generation certified')
     WHERE key = p_key
     RETURNING ready INTO is_ready;

    RETURN coalesce(is_ready, false);
END;
$$;

REVOKE ALL ON FUNCTION markhand_pending_reconcile_jobs() FROM PUBLIC;
REVOKE ALL ON FUNCTION markhand_runtime_readiness_open(text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION markhand_runtime_readiness_try_ready(text, text) FROM PUBLIC;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'markhand_app') THEN
        GRANT EXECUTE ON FUNCTION markhand_pending_reconcile_jobs() TO markhand_app;
        GRANT EXECUTE ON FUNCTION markhand_runtime_readiness_open(text, text) TO markhand_app;
        GRANT EXECUTE ON FUNCTION markhand_runtime_readiness_try_ready(text, text) TO markhand_app;
    END IF;
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'markhand_test') THEN
        GRANT EXECUTE ON FUNCTION markhand_pending_reconcile_jobs() TO markhand_test;
        GRANT EXECUTE ON FUNCTION markhand_runtime_readiness_open(text, text) TO markhand_test;
        GRANT EXECUTE ON FUNCTION markhand_runtime_readiness_try_ready(text, text) TO markhand_test;
    END IF;
    -- Migration/session owner retains execute by virtue of ownership.
END
$$;
