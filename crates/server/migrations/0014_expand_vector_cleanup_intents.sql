-- Phase: 1B
-- Owner: worker-owner, retrieval-owner
-- Change: expand
-- Lock/data risk: creates empty cleanup-intent table + RLS policy.
-- Rollback compatibility: additive; drop table to reverse.
--
-- Durable intent for Qdrant writes so a kill between upsert and compensation
-- cannot leave unaudited/unrecoverable orphan vectors after purge.

CREATE TABLE vector_cleanup_intents (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    document_id uuid NOT NULL,
    job_id uuid NOT NULL,
    index_signature_sha256 text NOT NULL
        CHECK (index_signature_sha256 ~ '^[0-9a-f]{64}$'),
    point_ids uuid[] NOT NULL CHECK (cardinality(point_ids) > 0),
    status text NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'completed')),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_vector_cleanup_intents__org_job UNIQUE (org_id, job_id),
    CONSTRAINT fk_vector_cleanup_intents__document_org
        FOREIGN KEY (org_id, document_id) REFERENCES documents (org_id, id)
        ON DELETE CASCADE,
    CONSTRAINT fk_vector_cleanup_intents__job_org
        FOREIGN KEY (org_id, job_id) REFERENCES jobs (org_id, id)
        ON DELETE CASCADE
);

CREATE INDEX idx_vector_cleanup_intents__org_document_pending
    ON vector_cleanup_intents (org_id, document_id)
    WHERE status = 'pending';

ALTER TABLE vector_cleanup_intents ENABLE ROW LEVEL SECURITY;
ALTER TABLE vector_cleanup_intents FORCE ROW LEVEL SECURITY;
CREATE POLICY vector_cleanup_intents_org_isolation ON vector_cleanup_intents
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());
