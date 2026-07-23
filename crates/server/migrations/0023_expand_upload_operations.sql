-- Phase: 1B
-- Owner: storage-owner, ingest-owner
-- Change: expand
-- Lock/data risk: creates empty upload_operations table + RLS + review permission; no backfill.
-- Rollback compatibility: additive; drop table / permission row to reverse.
--
-- Durable upload idempotency + crash reconciliation for MinIO↔DB.
-- States: started → reserved → putting → object_stored → completed
--         object_stored → reconciling → refunded | cleanup_pending
--         stale started/putting/reserved → refunded via reconciler.
-- Terminal: completed (rows + finalized quota) OR refunded/cleanup_pending
-- (no document/job side effects, object deleted or durable cleanup).

CREATE TABLE upload_operations (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    idempotency_key text NOT NULL
        CHECK (char_length(idempotency_key) BETWEEN 1 AND 160),
    -- Canonical request envelope (bytes SHA + collection + normalized metadata).
    envelope_sha256 text NOT NULL
        CHECK (envelope_sha256 ~ '^[a-f0-9]{64}$'),
    content_sha256 text NOT NULL
        CHECK (content_sha256 ~ '^[a-f0-9]{64}$'),
    state text NOT NULL DEFAULT 'started'
        CHECK (state IN (
            'started',
            'reserved',
            'putting',
            'object_stored',
            'reconciling',
            'cleanup_pending',
            'completed',
            'refunded',
            'failed'
        )),
    attempt integer NOT NULL DEFAULT 1 CHECK (attempt >= 1),
    reservation_key text NOT NULL
        CHECK (char_length(reservation_key) BETWEEN 1 AND 200),
    expected_object_key text,
    object_key text,
    object_id uuid NOT NULL,
    collection_id uuid NOT NULL,
    document_id uuid NOT NULL,
    version_id uuid NOT NULL,
    job_id uuid,
    disposition text
        CHECK (disposition IS NULL OR disposition IN ('accepted', 'quarantined')),
    -- Stable response fields for deep-equality replay (requestId is volatile).
    size_bytes bigint CHECK (size_bytes IS NULL OR size_bytes >= 0),
    canonical_format text,
    original_filename text,
    threat_class text,
    reason_code text,
    -- Quarantine review metadata (approval path).
    reviewed_by_user_id uuid REFERENCES users(id) ON DELETE RESTRICT,
    reviewed_at timestamptz,
    review_reason text,
    error_code text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_upload_operations__org_user_key
        UNIQUE (org_id, user_id, idempotency_key)
);

CREATE INDEX idx_upload_operations__org_state_updated
    ON upload_operations (org_id, state, updated_at);

CREATE INDEX idx_upload_operations__org_document
    ON upload_operations (org_id, document_id);

CREATE INDEX idx_upload_operations__org_collection_document
    ON upload_operations (org_id, collection_id, document_id);

ALTER TABLE upload_operations ENABLE ROW LEVEL SECURITY;
ALTER TABLE upload_operations FORCE ROW LEVEL SECURITY;
CREATE POLICY upload_operations_org_isolation ON upload_operations
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

-- Reviewer permission for quarantined intake approval (not granted to default uploader).
INSERT INTO permissions (id, code, description)
VALUES (
    '33333333-3333-3333-3333-333333333310',
    'doc.quarantine.review',
    'Approve quarantined uploads for conversion'
)
ON CONFLICT (code) DO NOTHING;
