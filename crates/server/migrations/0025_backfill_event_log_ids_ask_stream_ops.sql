-- Phase: 1B
-- Owner: api-owner
-- Change: expand
-- Lock/data risk: one-shot UPDATE of legacy event_log ID columns from payload;
--   ask_stream producer lease columns + expires index for SKIP LOCKED purge.
-- Rollback compatibility: additive columns/nullable; backfill is idempotent.
--
-- 1) Backfill legacy event_log.job_id/document_id/version_id from validated
--    payload UUID fields when columns were left NULL by older writers.
--    Canonical payload keys are snake_case (job_id/document_id/version_id);
--    camelCase (jobId/documentId/versionId) is accepted for compatibility.
-- 2) Ask-stream producer lease + purge support for R05 lifecycle/retention.

-- Backfill only when payload values are valid UUID text and FKs still exist.
-- Prefer snake_case keys; accept camelCase for older writers.
UPDATE event_log e
SET job_id = CASE
        WHEN e.job_id IS NULL
             AND e.payload ? 'job_id'
             AND e.payload->>'job_id' ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
             AND EXISTS (
                 SELECT 1 FROM jobs j
                 WHERE j.org_id = e.org_id
                   AND j.id = (e.payload->>'job_id')::uuid
             )
        THEN (e.payload->>'job_id')::uuid
        WHEN e.job_id IS NULL
             AND e.payload ? 'jobId'
             AND e.payload->>'jobId' ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
             AND EXISTS (
                 SELECT 1 FROM jobs j
                 WHERE j.org_id = e.org_id
                   AND j.id = (e.payload->>'jobId')::uuid
             )
        THEN (e.payload->>'jobId')::uuid
        ELSE e.job_id
    END,
    document_id = CASE
        WHEN e.document_id IS NULL
             AND e.payload ? 'document_id'
             AND e.payload->>'document_id' ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
             AND EXISTS (
                 SELECT 1 FROM documents d
                 WHERE d.org_id = e.org_id
                   AND d.id = (e.payload->>'document_id')::uuid
             )
        THEN (e.payload->>'document_id')::uuid
        WHEN e.document_id IS NULL
             AND e.payload ? 'documentId'
             AND e.payload->>'documentId' ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
             AND EXISTS (
                 SELECT 1 FROM documents d
                 WHERE d.org_id = e.org_id
                   AND d.id = (e.payload->>'documentId')::uuid
             )
        THEN (e.payload->>'documentId')::uuid
        ELSE e.document_id
    END
WHERE e.job_id IS NULL OR e.document_id IS NULL;

UPDATE event_log e
SET version_id = CASE
        WHEN e.version_id IS NULL
             AND e.document_id IS NOT NULL
             AND e.payload ? 'version_id'
             AND e.payload->>'version_id' ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
             AND EXISTS (
                 SELECT 1 FROM document_versions v
                 WHERE v.org_id = e.org_id
                   AND v.document_id = e.document_id
                   AND v.id = (e.payload->>'version_id')::uuid
             )
        THEN (e.payload->>'version_id')::uuid
        WHEN e.version_id IS NULL
             AND e.document_id IS NOT NULL
             AND e.payload ? 'versionId'
             AND e.payload->>'versionId' ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
             AND EXISTS (
                 SELECT 1 FROM document_versions v
                 WHERE v.org_id = e.org_id
                   AND v.document_id = e.document_id
                   AND v.id = (e.payload->>'versionId')::uuid
             )
        THEN (e.payload->>'versionId')::uuid
        ELSE e.version_id
    END
WHERE e.version_id IS NULL
  AND e.document_id IS NOT NULL;

ALTER TABLE ask_stream_sessions
    ADD COLUMN IF NOT EXISTS producer_lease_until timestamptz,
    ADD COLUMN IF NOT EXISTS producer_epoch integer NOT NULL DEFAULT 0
        CHECK (producer_epoch >= 0);

CREATE INDEX IF NOT EXISTS idx_ask_stream_sessions__purge_expired
    ON ask_stream_sessions (expires_at)
    WHERE expires_at IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_ask_stream_sessions__producer_lease
    ON ask_stream_sessions (producer_lease_until)
    WHERE status = 'open' AND producer_lease_until IS NOT NULL;

-- Cross-org purge under FORCE RLS (app role). Cascades events via FK.
CREATE OR REPLACE FUNCTION markhand_purge_expired_ask_streams(p_limit integer)
RETURNS TABLE(sessions_purged bigint, events_purged bigint)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    v_limit integer := GREATEST(1, LEAST(COALESCE(p_limit, 100), 500));
BEGIN
    RETURN QUERY
    WITH doomed AS (
        SELECT s.id
        FROM public.ask_stream_sessions s
        WHERE s.expires_at <= clock_timestamp()
        ORDER BY s.expires_at ASC
        FOR UPDATE SKIP LOCKED
        LIMIT v_limit
    ),
    deleted_events AS (
        DELETE FROM public.ask_stream_events e
        USING doomed d
        WHERE e.session_id = d.id
        RETURNING e.id
    ),
    deleted_sessions AS (
        DELETE FROM public.ask_stream_sessions s
        USING doomed d
        WHERE s.id = d.id
        RETURNING s.id
    )
    SELECT
        (SELECT count(*)::bigint FROM deleted_sessions),
        (SELECT count(*)::bigint FROM deleted_events);
END;
$$;

REVOKE ALL ON FUNCTION markhand_purge_expired_ask_streams(integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION markhand_purge_expired_ask_streams(integer) TO markhand_app;

-- Cross-org stale producer recovery (durable terminal once per session).
CREATE OR REPLACE FUNCTION markhand_recover_stale_ask_stream_producers(p_limit integer)
RETURNS bigint
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    v_limit integer := GREATEST(1, LEAST(COALESCE(p_limit, 50), 100));
    r record;
    recovered bigint := 0;
    seq bigint;
BEGIN
    FOR r IN
        SELECT s.id, s.org_id, s.user_id
        FROM public.ask_stream_sessions s
        WHERE s.status = 'open'
          AND s.producer_lease_until IS NOT NULL
          AND s.producer_lease_until < clock_timestamp()
        ORDER BY s.producer_lease_until ASC
        FOR UPDATE SKIP LOCKED
        LIMIT v_limit
    LOOP
        -- Skip if a terminal already exists (exact-one guard).
        IF EXISTS (
            SELECT 1 FROM public.ask_stream_events e
            WHERE e.org_id = r.org_id
              AND e.session_id = r.id
              AND e.event_type = 'stream.closed'
        ) THEN
            UPDATE public.ask_stream_sessions
            SET status = 'error',
                close_reason = 'producer_lost',
                closed_at = clock_timestamp(),
                producer_lease_until = NULL
            WHERE org_id = r.org_id AND id = r.id AND status = 'open';
            recovered := recovered + 1;
            CONTINUE;
        END IF;

        SELECT next_sequence INTO seq
        FROM public.ask_stream_sessions
        WHERE org_id = r.org_id AND id = r.id
        FOR UPDATE;

        INSERT INTO public.ask_stream_events (
            org_id, session_id, user_id, sequence_no, event_type,
            envelope_version, data, payload_bytes
        ) VALUES (
            r.org_id, r.id, r.user_id, seq, 'stream.closed',
            1,
            jsonb_build_object('reason', 'producer_lost', 'streamSessionId', r.id),
            64
        );

        UPDATE public.ask_stream_sessions
        SET next_sequence = seq + 1,
            event_count = event_count + 1,
            status = 'error',
            close_reason = 'producer_lost',
            closed_at = clock_timestamp(),
            producer_lease_until = NULL
        WHERE org_id = r.org_id AND id = r.id AND status = 'open';

        recovered := recovered + 1;
    END LOOP;
    RETURN recovered;
END;
$$;

REVOKE ALL ON FUNCTION markhand_recover_stale_ask_stream_producers(integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION markhand_recover_stale_ask_stream_producers(integer) TO markhand_app;
