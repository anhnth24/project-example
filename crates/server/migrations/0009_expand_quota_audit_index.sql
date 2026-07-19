-- Phase: 1B
-- Owner: storage-owner, operations-owner
-- Change: expand
-- Lock/data risk: creates empty quota/audit tables and indexes.
-- Rollback compatibility: additive only; counters start at zero.
-- Quota reservation tables and append-oriented audit log.
-- (index_metadata lives in 0006 so chunks can FK to a concrete generation.)

CREATE TABLE org_quotas (
    org_id uuid PRIMARY KEY REFERENCES orgs(id) ON DELETE RESTRICT,
    max_storage_bytes bigint NOT NULL CHECK (max_storage_bytes >= 0),
    max_documents integer NOT NULL CHECK (max_documents >= 0),
    max_concurrent_jobs integer NOT NULL CHECK (max_concurrent_jobs >= 0),
    max_monthly_tokens bigint NOT NULL CHECK (max_monthly_tokens >= 0),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE usage_counters (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    counter_key text NOT NULL CHECK (counter_key ~ '^[a-z][a-z0-9_.]{1,63}$'),
    period_start timestamptz NOT NULL,
    period_end timestamptz NOT NULL,
    value bigint NOT NULL DEFAULT 0 CHECK (value >= 0),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_usage_counters__org_key_period UNIQUE (org_id, counter_key, period_start),
    CONSTRAINT ck_usage_counters__period CHECK (period_end > period_start)
);

CREATE INDEX idx_usage_counters__org_period ON usage_counters (org_id, period_start, period_end);

CREATE TABLE quota_reservations (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    reservation_key text NOT NULL,
    resource_kind text NOT NULL CHECK (resource_kind IN (
        'storage_bytes',
        'documents',
        'concurrent_jobs',
        'tokens'
    )),
    amount bigint NOT NULL CHECK (amount > 0),
    status text NOT NULL DEFAULT 'reserved'
        CHECK (status IN ('reserved', 'finalized', 'refunded', 'expired')),
    expires_at timestamptz NOT NULL,
    job_id uuid,
    created_at timestamptz NOT NULL DEFAULT now(),
    settled_at timestamptz,
    CONSTRAINT uq_quota_reservations__org_key UNIQUE (org_id, reservation_key),
    CONSTRAINT fk_quota_reservations__job_org
        FOREIGN KEY (org_id, job_id) REFERENCES jobs (org_id, id)
        ON DELETE SET NULL (job_id)
);

CREATE INDEX idx_quota_reservations__org_status ON quota_reservations (org_id, status, expires_at);

CREATE TABLE audit_log (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    seq bigserial NOT NULL,
    actor_user_id uuid REFERENCES users(id) ON DELETE SET NULL,
    action text NOT NULL CHECK (length(trim(action)) > 0),
    resource_type text NOT NULL CHECK (length(trim(resource_type)) > 0),
    resource_id text,
    outcome text NOT NULL CHECK (outcome IN ('success', 'deny', 'error')),
    metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
    request_id text,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_audit_log__seq UNIQUE (seq)
);

CREATE INDEX idx_audit_log__org_created ON audit_log (org_id, created_at);
CREATE INDEX idx_audit_log__org_action ON audit_log (org_id, action);
