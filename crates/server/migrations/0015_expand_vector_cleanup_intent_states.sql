-- Phase: 1B
-- Owner: worker-owner, retrieval-owner
-- Change: expand
-- Lock/data risk: short CHECK rewrite + status remap; table is small/new.
-- Rollback compatibility: remap cleaned/committed/writing back to completed/pending.
--
-- Split ambiguous "completed" into cleaned (cleanup won) vs committed (write won)
-- so a drained intent can never be revived into another upsert.

ALTER TABLE vector_cleanup_intents
    DROP CONSTRAINT IF EXISTS vector_cleanup_intents_status_check;

UPDATE vector_cleanup_intents
SET status = 'committed'
WHERE status = 'completed';

ALTER TABLE vector_cleanup_intents
    ADD CONSTRAINT vector_cleanup_intents_status_check
    CHECK (status IN ('pending', 'writing', 'cleaned', 'committed'));

DROP INDEX IF EXISTS idx_vector_cleanup_intents__org_document_pending;
CREATE INDEX idx_vector_cleanup_intents__org_document_open
    ON vector_cleanup_intents (org_id, document_id)
    WHERE status IN ('pending', 'writing');
