-- Phase: 1B
-- Owner: api-owner
-- Change: expand
-- Lock/data risk: creates empty API idempotency table + RLS; no hot-path locks.
-- Rollback compatibility: additive only; drop table if unused.
-- HTTP Idempotency-Key claim/finalize store for upload/reindex (org+user+scope scoped).
-- Durable states: in_progress (claim before side effects) → completed (exact replay).

CREATE TABLE api_idempotency_keys (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    scope text NOT NULL CHECK (scope IN ('upload', 'reindex')),
    idempotency_key text NOT NULL
        CHECK (char_length(idempotency_key) BETWEEN 1 AND 128),
    state text NOT NULL CHECK (state IN ('in_progress', 'completed')),
    request_hash text NOT NULL CHECK (request_hash ~ '^[a-f0-9]{64}$'),
    response_status integer
        CHECK (response_status IS NULL OR response_status BETWEEN 100 AND 599),
    response_body jsonb,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    CONSTRAINT uq_api_idempotency_keys__scope_key
        UNIQUE (org_id, user_id, scope, idempotency_key),
    CONSTRAINT ck_api_idempotency_keys__completed_response CHECK (
        (state = 'in_progress'
            AND response_status IS NULL
            AND response_body IS NULL)
        OR (state = 'completed'
            AND response_status IS NOT NULL
            AND response_body IS NOT NULL)
    )
);

CREATE INDEX idx_api_idempotency_keys__org_created
    ON api_idempotency_keys (org_id, created_at);

CREATE INDEX idx_api_idempotency_keys__org_expires
    ON api_idempotency_keys (org_id, expires_at)
    WHERE state = 'in_progress';

ALTER TABLE api_idempotency_keys ENABLE ROW LEVEL SECURITY;
ALTER TABLE api_idempotency_keys FORCE ROW LEVEL SECURITY;
CREATE POLICY api_idempotency_keys_org_isolation ON api_idempotency_keys
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());
