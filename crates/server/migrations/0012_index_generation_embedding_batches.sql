-- Phase: 1B
-- Owner: retrieval-owner, worker-owner
-- Change: expand
-- Lock/data risk: short metadata/chunk-index lock; batch tables are initially empty.
-- Rollback compatibility: additive generation lifecycle and durable embedding work.
-- ADR 0011: expand → backfill → shadow → operator cutover → contract.

ALTER TABLE index_metadata
    ADD COLUMN state text NOT NULL DEFAULT 'active'
        CHECK (state IN ('building', 'shadow', 'active', 'draining', 'retired'));

-- Existing deployments only have active metadata at the time this migration is
-- applied. Keep the legacy boolean pointer and lifecycle state mutually useful
-- until the old column can be contracted in a later release.
UPDATE index_metadata
SET state = CASE WHEN is_active THEN 'active' ELSE 'retired' END;

ALTER TABLE index_metadata
    DROP CONSTRAINT uq_index_metadata__org_signature_generation;
CREATE UNIQUE INDEX uq_index_metadata__org_collection_signature_generation
    ON index_metadata (
        org_id,
        coalesce(collection_id, '00000000-0000-0000-0000-000000000000'::uuid),
        index_signature_sha256,
        generation
    );

CREATE INDEX idx_index_metadata__org_collection_state
    ON index_metadata (org_id, collection_id, state);

-- The original identity uniqueness was global and prevented a shadow generation
-- from retaining its own immutable chunk catalog. Scope identities/ordinals to
-- their generation so a backfill can coexist with the active generation.
ALTER TABLE chunks DROP CONSTRAINT uq_chunks__identity;
ALTER TABLE chunks DROP CONSTRAINT uq_chunks__version_ordinal;
ALTER TABLE chunks
    ADD CONSTRAINT uq_chunks__generation_identity
        UNIQUE (org_id, index_metadata_id, chunk_identity_sha256);
ALTER TABLE chunks
    ADD CONSTRAINT uq_chunks__generation_version_ordinal
        UNIQUE (org_id, index_metadata_id, version_id, ordinal);

CREATE TABLE index_generation_backfills (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    index_metadata_id uuid NOT NULL,
    document_id uuid NOT NULL,
    version_id uuid NOT NULL,
    status text NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'indexing', 'backfilled', 'failed')),
    created_at timestamptz NOT NULL DEFAULT now(),
    completed_at timestamptz,
    CONSTRAINT uq_index_generation_backfills__org_generation_version
        UNIQUE (org_id, index_metadata_id, document_id, version_id),
    CONSTRAINT fk_index_generation_backfills__metadata_org
        FOREIGN KEY (org_id, index_metadata_id)
        REFERENCES index_metadata (org_id, id) ON DELETE RESTRICT,
    CONSTRAINT fk_index_generation_backfills__version_lineage
        FOREIGN KEY (org_id, document_id, version_id)
        REFERENCES document_versions (org_id, document_id, id) ON DELETE RESTRICT
);

CREATE INDEX idx_index_generation_backfills__org_generation_status
    ON index_generation_backfills (org_id, index_metadata_id, status);

CREATE TABLE embedding_batches (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    index_job_id uuid NOT NULL,
    job_id uuid NOT NULL,
    index_metadata_id uuid NOT NULL,
    document_id uuid NOT NULL,
    version_id uuid NOT NULL,
    start_ordinal integer NOT NULL CHECK (start_ordinal >= 0),
    end_ordinal integer NOT NULL CHECK (end_ordinal > start_ordinal),
    input_sha256 text NOT NULL CHECK (input_sha256 ~ '^[a-f0-9]{64}$'),
    status text NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'succeeded', 'failed')),
    created_at timestamptz NOT NULL DEFAULT now(),
    completed_at timestamptz,
    CONSTRAINT uq_embedding_batches__org_generation_version_range
        UNIQUE (org_id, index_metadata_id, document_id, version_id, start_ordinal, end_ordinal),
    CONSTRAINT fk_embedding_batches__index_job_org
        FOREIGN KEY (org_id, index_job_id) REFERENCES jobs (org_id, id) ON DELETE RESTRICT,
    CONSTRAINT fk_embedding_batches__job_org
        FOREIGN KEY (org_id, job_id) REFERENCES jobs (org_id, id) ON DELETE RESTRICT,
    CONSTRAINT fk_embedding_batches__metadata_org
        FOREIGN KEY (org_id, index_metadata_id)
        REFERENCES index_metadata (org_id, id) ON DELETE RESTRICT,
    CONSTRAINT fk_embedding_batches__version_lineage
        FOREIGN KEY (org_id, document_id, version_id)
        REFERENCES document_versions (org_id, document_id, id) ON DELETE RESTRICT
);

CREATE INDEX idx_embedding_batches__org_generation_status
    ON embedding_batches (org_id, index_metadata_id, status);
CREATE INDEX idx_embedding_batches__org_job
    ON embedding_batches (org_id, job_id);

-- A shadow generation is not visible until an explicit cutover transaction
-- flips the active pointer. The application owns those transitions because it
-- also checks retrieval/citation evidence before calling cutover.
