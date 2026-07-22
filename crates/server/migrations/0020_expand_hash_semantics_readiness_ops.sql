-- Phase: 1B
-- Owner: retrieval-owner, ops-owner
-- Change: expand
-- Lock/data risk: backfill content_sha256 where it wrongly equaled markdown artifact;
--                 adds SECURITY DEFINER aggregates for cross-org readiness under FORCE RLS.
-- Rollback compatibility: drop functions; content_sha256 backfill is additive repair.

-- ---------------------------------------------------------------------------
-- 1) Repair document_versions.content_sha256 that incorrectly stored Markdown SHA.
--    Prefer parent (upload/source) hash when version hash equals markdown artifact.
-- ---------------------------------------------------------------------------
UPDATE document_versions dv
SET content_sha256 = parent.content_sha256,
    source_content_type = COALESCE(dv.source_content_type, parent.source_content_type),
    byte_size = COALESCE(parent.byte_size, dv.byte_size)
FROM document_versions parent,
     derived_artifacts da
WHERE dv.parent_version_id = parent.id
  AND parent.org_id = dv.org_id
  AND da.org_id = dv.org_id
  AND da.version_id = dv.id
  AND da.artifact_kind = 'markdown'
  AND dv.content_sha256 = da.content_sha256
  AND parent.content_sha256 <> da.content_sha256;

-- Unrepairable rows (no usable parent) stay as-is; citation/preview fail-closed on mismatch.

-- ---------------------------------------------------------------------------
-- 2) Privileged readiness aggregates (FORCE RLS safe). Fixed search_path; revoke PUBLIC.
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION markhand_index_generation_consistent(p_signature text)
RETURNS boolean
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT CASE
        WHEN NOT EXISTS (SELECT 1 FROM public.index_metadata) THEN true
        WHEN EXISTS (
            SELECT 1
            FROM public.index_metadata
            WHERE is_active = true
              AND state = 'active'
              AND index_signature_sha256 <> p_signature
        ) THEN false
        WHEN EXISTS (
            SELECT 1
            FROM (
                SELECT DISTINCT org_id FROM public.index_metadata
            ) orgs
            WHERE NOT EXISTS (
                SELECT 1
                FROM public.index_metadata im
                WHERE im.org_id = orgs.org_id
                  AND im.is_active = true
                  AND im.state = 'active'
                  AND im.index_signature_sha256 = p_signature
            )
        ) THEN false
        ELSE true
    END;
$$;

CREATE OR REPLACE FUNCTION markhand_any_reconcile_running()
RETURNS boolean
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT EXISTS (
        SELECT 1
        FROM public.jobs
        WHERE job_type = 'reconcile'
          AND status IN ('pending', 'leased', 'running')
    );
$$;

CREATE OR REPLACE FUNCTION markhand_any_blocking_fence_active()
RETURNS boolean
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT EXISTS (
        SELECT 1
        FROM public.ops_fences
        WHERE active = true
          AND name = ANY (ARRAY['restore', 'reconcile']::text[])
    );
$$;

REVOKE ALL ON FUNCTION markhand_index_generation_consistent(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION markhand_any_reconcile_running() FROM PUBLIC;
REVOKE ALL ON FUNCTION markhand_any_blocking_fence_active() FROM PUBLIC;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'markhand_app') THEN
        GRANT EXECUTE ON FUNCTION markhand_index_generation_consistent(text) TO markhand_app;
        GRANT EXECUTE ON FUNCTION markhand_any_reconcile_running() TO markhand_app;
        GRANT EXECUTE ON FUNCTION markhand_any_blocking_fence_active() TO markhand_app;
    END IF;
END $$;
