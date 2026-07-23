-- Phase: 1B
-- Owner: storage-owner
-- Change: expand
-- Lock/data risk: widens jobs.job_type CHECK only; no row rewrite.
-- Rollback compatibility: reject new enqueues; existing lifecycle_refresh rows
--                         would need draining before restoring the old CHECK.
-- Durable lifecycle-refresh jobs refresh Qdrant is_current/is_effective markers
-- after version promotion without replaying superseded index jobs.

ALTER TABLE jobs
    DROP CONSTRAINT IF EXISTS jobs_job_type_check;

ALTER TABLE jobs
    ADD CONSTRAINT jobs_job_type_check CHECK (job_type IN (
        'convert',
        'index',
        'delete',
        'reconcile',
        'embedding_batch',
        'lifecycle_refresh'
    ));
