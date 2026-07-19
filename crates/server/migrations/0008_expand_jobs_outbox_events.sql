-- Phase: 1B
-- Owner: storage-owner
-- Change: expand
-- Lock/data risk: creates empty jobs/outbox/event tables and claim indexes.
-- Rollback compatibility: schema only; worker logic arrives in later issues.
-- Durable jobs, transactional outbox, and sequenced event log (schema only).
-- ON DELETE SET NULL (col) keeps NOT NULL org_id intact (PG15+ column-list form).

CREATE TABLE jobs (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    job_type text NOT NULL CHECK (job_type IN (
        'convert',
        'index',
        'delete',
        'reconcile',
        'embedding_batch'
    )),
    status text NOT NULL DEFAULT 'pending'
        CHECK (status IN (
            'pending',
            'leased',
            'running',
            'succeeded',
            'failed',
            'cancelled',
            'dead_letter'
        )),
    payload_version integer NOT NULL DEFAULT 1 CHECK (payload_version >= 1),
    payload jsonb NOT NULL DEFAULT '{}'::jsonb,
    attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    max_attempts integer NOT NULL DEFAULT 5 CHECK (max_attempts >= 1),
    lease_owner text,
    lease_expires_at timestamptz,
    heartbeat_at timestamptz,
    checkpoint jsonb,
    idempotency_key text NOT NULL,
    document_id uuid,
    version_id uuid,
    available_at timestamptz NOT NULL DEFAULT now(),
    started_at timestamptz,
    finished_at timestamptz,
    last_error text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_jobs__org_id_id UNIQUE (org_id, id),
    CONSTRAINT uq_jobs__org_idempotency UNIQUE (org_id, job_type, idempotency_key),
    CONSTRAINT ck_jobs__version_requires_document CHECK (
        version_id IS NULL OR document_id IS NOT NULL
    ),
    CONSTRAINT fk_jobs__document_org
        FOREIGN KEY (org_id, document_id) REFERENCES documents (org_id, id)
        ON DELETE SET NULL (document_id),
    CONSTRAINT fk_jobs__version_lineage
        FOREIGN KEY (org_id, document_id, version_id)
        REFERENCES document_versions (org_id, document_id, id)
        ON DELETE SET NULL (document_id, version_id)
);

CREATE INDEX idx_jobs__org_status_available ON jobs (org_id, status, available_at);
CREATE INDEX idx_jobs__org_lease ON jobs (org_id, lease_expires_at)
    WHERE status = 'leased';

CREATE TABLE outbox_events (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    event_type text NOT NULL CHECK (length(trim(event_type)) > 0),
    payload_version integer NOT NULL DEFAULT 1 CHECK (payload_version >= 1),
    payload jsonb NOT NULL DEFAULT '{}'::jsonb,
    idempotency_key text NOT NULL,
    job_id uuid,
    published_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_outbox_events__org_idempotency UNIQUE (org_id, event_type, idempotency_key),
    CONSTRAINT fk_outbox_events__job_org
        FOREIGN KEY (org_id, job_id) REFERENCES jobs (org_id, id)
        ON DELETE SET NULL (job_id)
);

CREATE INDEX idx_outbox_events__org_unpublished
    ON outbox_events (org_id, created_at)
    WHERE published_at IS NULL;

CREATE TABLE event_log (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    sequence_no bigint NOT NULL,
    event_type text NOT NULL CHECK (length(trim(event_type)) > 0),
    payload_version integer NOT NULL DEFAULT 1 CHECK (payload_version >= 1),
    payload jsonb NOT NULL DEFAULT '{}'::jsonb,
    job_id uuid,
    document_id uuid,
    version_id uuid,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_event_log__org_sequence UNIQUE (org_id, sequence_no),
    CONSTRAINT ck_event_log__version_requires_document CHECK (
        version_id IS NULL OR document_id IS NOT NULL
    ),
    CONSTRAINT fk_event_log__job_org
        FOREIGN KEY (org_id, job_id) REFERENCES jobs (org_id, id)
        ON DELETE SET NULL (job_id),
    CONSTRAINT fk_event_log__document_org
        FOREIGN KEY (org_id, document_id) REFERENCES documents (org_id, id)
        ON DELETE SET NULL (document_id),
    -- Same-document lineage when version is present (MATCH SIMPLE skips when version_id NULL).
    CONSTRAINT fk_event_log__version_lineage
        FOREIGN KEY (org_id, document_id, version_id)
        REFERENCES document_versions (org_id, document_id, id)
        ON DELETE SET NULL (document_id, version_id)
);

CREATE INDEX idx_event_log__org_created ON event_log (org_id, created_at);
