-- Phase: 1C
-- Owner: security-owner, operations-owner
-- Change: expand
-- Lock/data risk: GRANT/REVOKE only (catalog updates); no row rewrites, no table locks.
-- Rollback compatibility: grants can be revoked manually; the role itself is
--   provisioned/removed by deploy/ops, never by migrations.
-- 1C-08: dedicated least-privilege runtime role for background workers.
--
-- Role lifecycle follows the markhand_app precedent (0027): LOGIN roles are
-- created by deploy/ops (passwords never live in migrations); this migration
-- only applies grants when the role already exists. Environments without the
-- role migrate unchanged and their workers keep connecting as markhand_app
-- via the MARKHAND_WORKER_DATABASE_URL -> MARKHAND_DATABASE_URL fallback
-- (see crates/server/src/config.rs).
--
-- markhand_worker receives DML only on the tables the worker binary reaches
-- (crates/server/src/bin/worker.rs -> db::jobs / db::documents /
-- db::document_versions / db::chunks / db::claims / db::index_metadata /
-- db::embedding_batches / db::vector_cleanup_intents / db::quota /
-- services::audit):
--   * job queue + outbox/event relay,
--   * conversion/index pipeline state,
--   * quota accounting (reserve/finalize/refund; org_quotas read-only),
--   * append-only audit (SELECT+INSERT, like markhand_app).
-- It gets NO access at all to auth/session, membership/ACL, invite, chat
-- history, upload operation, or download capability tables (any table not
-- granted below stays unreachable). FORCE RLS still applies on top: the role
-- is not an owner and must be created NOSUPERUSER + NOBYPASSRLS (asserted by
-- DB-gated tests).

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'markhand_worker') THEN
        GRANT USAGE ON SCHEMA public TO markhand_worker;
        BEGIN
            EXECUTE 'REVOKE CREATE ON SCHEMA public FROM markhand_worker';
        EXCEPTION WHEN insufficient_privilege OR undefined_object THEN
            NULL;
        END;

        -- Job queue + outbox/event relay (claim/heartbeat/finish/reclaim).
        GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE
            jobs, outbox_events, event_log
            TO markhand_worker;

        -- Conversion pipeline state.
        GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE
            documents, document_versions, derived_artifacts, chunks, claims
            TO markhand_worker;

        -- Index/embedding generation + vector cleanup state.
        GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE
            index_metadata, index_generation_backfills, embedding_batches,
            vector_cleanup_intents
            TO markhand_worker;

        -- Quota accounting: reservations/counters DML, limits read-only.
        GRANT SELECT ON TABLE org_quotas TO markhand_worker;
        GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE
            quota_reservations, usage_counters
            TO markhand_worker;

        -- Append-only audit, same shape as markhand_app (0027/0028).
        GRANT SELECT, INSERT ON TABLE audit_log TO markhand_worker;
        REVOKE UPDATE, DELETE, TRUNCATE ON TABLE audit_log FROM markhand_worker;

        GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO markhand_worker;
        -- Includes SECURITY DEFINER helpers the worker loop calls
        -- (markhand_job_queue_stats) and RLS predicate helpers
        -- (markhand_current_org_id).
        GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA public TO markhand_worker;
    END IF;
END
$$;
