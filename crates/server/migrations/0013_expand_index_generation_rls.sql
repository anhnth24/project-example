-- Phase: 1B
-- Owner: retrieval-owner, worker-owner
-- Change: expand
-- Lock/data risk: short table locks to enable row-level security.
-- Rollback compatibility: additive tenant isolation for 0012 generation tables.
--
-- Kept as a new migration because 0012 is checksum-tracked and may already be
-- recorded in deployed databases. New tenant-scoped tables must receive the
-- same mandatory RLS policy as the existing business tables.

ALTER TABLE index_generation_backfills ENABLE ROW LEVEL SECURITY;
ALTER TABLE index_generation_backfills FORCE ROW LEVEL SECURITY;
CREATE POLICY index_generation_backfills_org_isolation ON index_generation_backfills
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

ALTER TABLE embedding_batches ENABLE ROW LEVEL SECURITY;
ALTER TABLE embedding_batches FORCE ROW LEVEL SECURITY;
CREATE POLICY embedding_batches_org_isolation ON embedding_batches
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());
