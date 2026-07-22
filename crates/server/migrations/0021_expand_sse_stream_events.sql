-- Phase: 1B
-- Owner: api-owner
-- Change: expand
-- Lock/data risk: creates empty SSE stream tables + RLS; short per-request
--   advisory lock only inside the atomic closed-snapshot transaction.
-- Rollback compatibility: additive only; drop tables if unused.
--
-- Resumable ask/SSE persistence (P1B-R05). Streams are written as a single
-- atomic closed snapshot (metadata + tokens + terminal). No durable open rows.
-- Auth scope (mode/history, collection IDs, cited doc/version IDs) is persisted
-- for reconnect revalidation. event_log remains org-wide job chronology.

CREATE TABLE sse_stream_requests (
    id uuid PRIMARY KEY,
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    kind text NOT NULL CHECK (kind IN ('ask')),
    status text NOT NULL CHECK (status IN ('closed', 'error', 'expired')),
    close_reason text NOT NULL
        CHECK (length(trim(close_reason)) > 0 AND length(close_reason) <= 64),
    version_mode text NOT NULL CHECK (version_mode IN (
        'current', 'as_of', 'compare', 'history'
    )),
    requires_history boolean NOT NULL,
    collection_ids uuid[] NOT NULL CHECK (cardinality(collection_ids) >= 1),
    cited_document_ids uuid[] NOT NULL DEFAULT '{}'::uuid[],
    cited_version_ids uuid[] NOT NULL DEFAULT '{}'::uuid[],
    next_sequence bigint NOT NULL CHECK (next_sequence >= 1),
    event_count integer NOT NULL CHECK (event_count >= 0),
    byte_count bigint NOT NULL CHECK (byte_count >= 0),
    max_events integer NOT NULL CHECK (max_events > 0 AND max_events <= 8192),
    max_bytes bigint NOT NULL CHECK (max_bytes > 0 AND max_bytes <= 1048576),
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    closed_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT ck_sse_stream_requests__expires_after_created
        CHECK (expires_at > created_at),
    CONSTRAINT uq_sse_stream_requests__org_id_user UNIQUE (org_id, id, user_id)
);

CREATE INDEX idx_sse_stream_requests__org_user_created
    ON sse_stream_requests (org_id, user_id, created_at DESC);

CREATE INDEX idx_sse_stream_requests__org_expires
    ON sse_stream_requests (org_id, expires_at);

CREATE TABLE sse_stream_events (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL,
    request_id uuid NOT NULL,
    user_id uuid NOT NULL,
    sequence_no bigint NOT NULL CHECK (sequence_no >= 1),
    event_type text NOT NULL CHECK (event_type IN (
        'metadata', 'token', 'close', 'error'
    )),
    envelope_version integer NOT NULL DEFAULT 1 CHECK (envelope_version >= 1),
    data jsonb NOT NULL DEFAULT '{}'::jsonb,
    payload_bytes integer NOT NULL CHECK (payload_bytes >= 0 AND payload_bytes <= 65536),
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_sse_stream_events__request_sequence
        UNIQUE (org_id, request_id, sequence_no),
    CONSTRAINT fk_sse_stream_events__request
        FOREIGN KEY (org_id, request_id, user_id)
        REFERENCES sse_stream_requests (org_id, id, user_id)
        ON DELETE CASCADE
);

CREATE INDEX idx_sse_stream_events__request_sequence
    ON sse_stream_events (org_id, request_id, sequence_no);

ALTER TABLE sse_stream_requests ENABLE ROW LEVEL SECURITY;
ALTER TABLE sse_stream_requests FORCE ROW LEVEL SECURITY;
CREATE POLICY sse_stream_requests_org_isolation ON sse_stream_requests
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

ALTER TABLE sse_stream_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE sse_stream_events FORCE ROW LEVEL SECURITY;
CREATE POLICY sse_stream_events_org_isolation ON sse_stream_events
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());
