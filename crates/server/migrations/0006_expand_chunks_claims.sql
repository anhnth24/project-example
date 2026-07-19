-- Phase: 1B
-- Owner: storage-owner, retrieval-owner
-- Change: expand
-- Lock/data risk: creates index_metadata, chunk/claim tables and GIN FTS index on empty table.
-- Rollback compatibility: additive only; no released readers depend on these tables yet.
-- Index generations (ADR 0006), version-scoped chunks with tsvector FTS, normalized typed claims.

CREATE TABLE index_metadata (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    collection_id uuid,
    index_signature_sha256 text NOT NULL CHECK (index_signature_sha256 ~ '^[a-f0-9]{64}$'),
    identity_version integer NOT NULL DEFAULT 2 CHECK (identity_version >= 1),
    chunking_version text NOT NULL DEFAULT 'heading-chunks-2000-v1',
    body_text_version text NOT NULL DEFAULT 'nfc-v1',
    query_normalization_version text NOT NULL DEFAULT 'accent-fold-v1',
    embedding_family text NOT NULL,
    embedding_revision text NOT NULL,
    dimensions integer NOT NULL CHECK (dimensions > 0),
    normalized boolean NOT NULL DEFAULT true,
    runtime_path text NOT NULL CHECK (runtime_path IN (
        'local-hash',
        'local-neural',
        'glm-cloud-interim',
        'vllm-local',
        'provider-cloud'
    )),
    generation integer NOT NULL DEFAULT 1 CHECK (generation >= 1),
    is_active boolean NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_index_metadata__org_id_id UNIQUE (org_id, id),
    CONSTRAINT uq_index_metadata__org_signature_generation
        UNIQUE (org_id, index_signature_sha256, generation),
    CONSTRAINT fk_index_metadata__collection_org
        FOREIGN KEY (org_id, collection_id) REFERENCES collections(org_id, id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX uq_index_metadata__org_active
    ON index_metadata (org_id, coalesce(collection_id, '00000000-0000-0000-0000-000000000000'::uuid))
    WHERE is_active;

CREATE INDEX idx_index_metadata__org_collection ON index_metadata (org_id, collection_id);

-- Identity/signature/generation/compatibility dimensions are immutable; only is_active may change.
CREATE OR REPLACE FUNCTION index_metadata_enforce_immutability()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'index_metadata: DELETE is forbidden (retire via is_active=false)'
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    IF NEW.id IS DISTINCT FROM OLD.id
        OR NEW.org_id IS DISTINCT FROM OLD.org_id
        OR NEW.collection_id IS DISTINCT FROM OLD.collection_id
        OR NEW.index_signature_sha256 IS DISTINCT FROM OLD.index_signature_sha256
        OR NEW.identity_version IS DISTINCT FROM OLD.identity_version
        OR NEW.chunking_version IS DISTINCT FROM OLD.chunking_version
        OR NEW.body_text_version IS DISTINCT FROM OLD.body_text_version
        OR NEW.query_normalization_version IS DISTINCT FROM OLD.query_normalization_version
        OR NEW.embedding_family IS DISTINCT FROM OLD.embedding_family
        OR NEW.embedding_revision IS DISTINCT FROM OLD.embedding_revision
        OR NEW.dimensions IS DISTINCT FROM OLD.dimensions
        OR NEW.normalized IS DISTINCT FROM OLD.normalized
        OR NEW.runtime_path IS DISTINCT FROM OLD.runtime_path
        OR NEW.generation IS DISTINCT FROM OLD.generation
        OR NEW.created_at IS DISTINCT FROM OLD.created_at
    THEN
        RAISE EXCEPTION 'index_metadata: identity/signature/generation columns are immutable'
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER trg_index_metadata__immutability
    BEFORE UPDATE OR DELETE ON index_metadata
    FOR EACH ROW
    EXECUTE FUNCTION index_metadata_enforce_immutability();

CREATE TABLE chunks (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    document_id uuid NOT NULL,
    version_id uuid NOT NULL,
    ordinal integer NOT NULL CHECK (ordinal >= 0),
    heading_path text[] NOT NULL DEFAULT '{}',
    body text NOT NULL CHECK (length(body) > 0),
    body_text_version text NOT NULL DEFAULT 'nfc-v1',
    chunk_identity_sha256 text NOT NULL CHECK (chunk_identity_sha256 ~ '^[a-f0-9]{64}$'),
    index_metadata_id uuid NOT NULL,
    index_signature text NOT NULL CHECK (index_signature ~ '^[a-f0-9]{64}$'),
    page integer CHECK (page IS NULL OR page >= 1),
    slide integer CHECK (slide IS NULL OR slide >= 1),
    sheet text,
    span_start integer CHECK (span_start IS NULL OR span_start >= 0),
    span_end integer CHECK (span_end IS NULL OR span_end >= 0),
    tsv tsvector NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_chunks__org_id_id UNIQUE (org_id, id),
    CONSTRAINT uq_chunks__org_document_version_id UNIQUE (org_id, document_id, version_id, id),
    CONSTRAINT uq_chunks__version_ordinal UNIQUE (version_id, ordinal),
    CONSTRAINT uq_chunks__identity UNIQUE (chunk_identity_sha256),
    CONSTRAINT ck_chunks__span CHECK (
        span_start IS NULL OR span_end IS NULL OR span_end >= span_start
    ),
    CONSTRAINT fk_chunks__version_lineage
        FOREIGN KEY (org_id, document_id, version_id)
        REFERENCES document_versions (org_id, document_id, id) ON DELETE RESTRICT,
    CONSTRAINT fk_chunks__index_metadata_org
        FOREIGN KEY (org_id, index_metadata_id)
        REFERENCES index_metadata (org_id, id) ON DELETE RESTRICT
);

CREATE INDEX idx_chunks__org_version ON chunks (org_id, version_id);
CREATE INDEX idx_chunks__org_document ON chunks (org_id, document_id);
CREATE INDEX idx_chunks__org_index_metadata ON chunks (org_id, index_metadata_id);
CREATE INDEX idx_chunks__org_index_signature ON chunks (org_id, index_signature);
CREATE INDEX idx_chunks__tsv ON chunks USING gin (tsv);

CREATE OR REPLACE FUNCTION chunks_set_tsv()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    NEW.tsv := to_tsvector('simple', coalesce(array_to_string(NEW.heading_path, ' '), '') || ' ' || NEW.body);
    RETURN NEW;
END;
$$;

CREATE TRIGGER trg_chunks__set_tsv
    BEFORE INSERT OR UPDATE OF heading_path, body ON chunks
    FOR EACH ROW
    EXECUTE FUNCTION chunks_set_tsv();

CREATE OR REPLACE FUNCTION chunks_enforce_index_generation()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    meta_sig text;
BEGIN
    SELECT index_signature_sha256 INTO meta_sig
    FROM index_metadata
    WHERE org_id = NEW.org_id AND id = NEW.index_metadata_id;

    IF meta_sig IS NULL THEN
        RAISE EXCEPTION 'chunks: index_metadata % not found in org %', NEW.index_metadata_id, NEW.org_id
            USING ERRCODE = 'foreign_key_violation';
    END IF;
    IF NEW.index_signature IS DISTINCT FROM meta_sig THEN
        RAISE EXCEPTION 'chunks: index_signature must equal index_metadata.index_signature_sha256'
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER trg_chunks__index_generation
    BEFORE INSERT OR UPDATE OF index_metadata_id, index_signature, org_id ON chunks
    FOR EACH ROW
    EXECUTE FUNCTION chunks_enforce_index_generation();

CREATE TABLE claims (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    document_id uuid NOT NULL,
    version_id uuid NOT NULL,
    chunk_id uuid,
    claim_key text NOT NULL CHECK (length(trim(claim_key)) > 0),
    subject text NOT NULL CHECK (length(trim(subject)) > 0),
    predicate text NOT NULL CHECK (length(trim(predicate)) > 0),
    value_type text NOT NULL CHECK (value_type IN (
        'number',
        'enum',
        'date',
        'boolean',
        'text',
        'money'
    )),
    value_number numeric,
    value_text text,
    value_boolean boolean,
    value_date date,
    value_money numeric,
    unit text,
    scope text NOT NULL DEFAULT '',
    effective_from timestamptz NOT NULL,
    effective_to timestamptz,
    citation_quote text,
    citation_span_start integer,
    citation_span_end integer,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_claims__org_id_id UNIQUE (org_id, id),
    CONSTRAINT ck_claims__effective_range CHECK (
        effective_to IS NULL OR effective_to > effective_from
    ),
    CONSTRAINT ck_claims__typed_value CHECK (
        (
            value_type = 'number'
            AND value_number IS NOT NULL
            AND value_text IS NULL AND value_boolean IS NULL
            AND value_date IS NULL AND value_money IS NULL
        ) OR (
            value_type = 'money'
            AND value_money IS NOT NULL
            AND value_number IS NULL AND value_text IS NULL
            AND value_boolean IS NULL AND value_date IS NULL
        ) OR (
            value_type = 'date'
            AND value_date IS NOT NULL
            AND value_number IS NULL AND value_text IS NULL
            AND value_boolean IS NULL AND value_money IS NULL
        ) OR (
            value_type = 'boolean'
            AND value_boolean IS NOT NULL
            AND value_number IS NULL AND value_text IS NULL
            AND value_date IS NULL AND value_money IS NULL
        ) OR (
            value_type IN ('enum', 'text')
            AND value_text IS NOT NULL
            AND value_number IS NULL AND value_boolean IS NULL
            AND value_date IS NULL AND value_money IS NULL
        )
    ),
    CONSTRAINT fk_claims__version_lineage
        FOREIGN KEY (org_id, document_id, version_id)
        REFERENCES document_versions (org_id, document_id, id) ON DELETE RESTRICT,
    -- Chunk citation must be same org+document+version lineage.
    CONSTRAINT fk_claims__chunk_lineage
        FOREIGN KEY (org_id, document_id, version_id, chunk_id)
        REFERENCES chunks (org_id, document_id, version_id, id)
        ON DELETE SET NULL (chunk_id)
);

CREATE INDEX idx_claims__org_version ON claims (org_id, version_id);
CREATE INDEX idx_claims__org_key_scope ON claims (org_id, claim_key, scope);
CREATE INDEX idx_claims__org_document ON claims (org_id, document_id);
