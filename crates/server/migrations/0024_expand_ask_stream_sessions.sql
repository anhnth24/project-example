-- Phase: 1B
-- Owner: api-owner
-- Change: expand
-- Lock/data risk: creates empty ask-stream tables + RLS; per-session advisory
--   lock only while appending the next monotonic sequence.
-- Rollback compatibility: additive only; drop tables if unused.
--
-- Resumable ask SSE (P1B-R05). Sessions pin a retrieval/version snapshot once.
-- Events append durably with monotonic sequence. Reconnect uses Last-Event-ID
-- against the same session and never re-runs retrieval/provider. Job chronology
-- remains in event_log.

CREATE TABLE ask_stream_sessions (
    id uuid PRIMARY KEY,
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    status text NOT NULL CHECK (status IN ('open', 'closed', 'error')),
    close_reason text
        CHECK (
            close_reason IS NULL
            OR (length(trim(close_reason)) > 0 AND length(close_reason) <= 64)
        ),
    version_mode text NOT NULL CHECK (version_mode IN (
        'current', 'as_of', 'compare', 'history'
    )),
    collection_ids uuid[] NOT NULL DEFAULT '{}'::uuid[],
    cited_document_ids uuid[] NOT NULL DEFAULT '{}'::uuid[],
    cited_version_ids uuid[] NOT NULL DEFAULT '{}'::uuid[],
    pinned_snapshot jsonb NOT NULL DEFAULT '{}'::jsonb,
    next_sequence bigint NOT NULL DEFAULT 1 CHECK (next_sequence >= 1),
    event_count integer NOT NULL DEFAULT 0 CHECK (event_count >= 0),
    byte_count bigint NOT NULL DEFAULT 0 CHECK (byte_count >= 0),
    max_events integer NOT NULL DEFAULT 4200
        CHECK (max_events > 0 AND max_events <= 8192),
    max_bytes bigint NOT NULL DEFAULT 262144
        CHECK (max_bytes > 0 AND max_bytes <= 1048576),
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    closed_at timestamptz,
    CONSTRAINT ck_ask_stream_sessions__expires_after_created
        CHECK (expires_at > created_at),
    CONSTRAINT ck_ask_stream_sessions__closed_reason
        CHECK (
            (status = 'open' AND close_reason IS NULL AND closed_at IS NULL)
            OR (status <> 'open' AND close_reason IS NOT NULL AND closed_at IS NOT NULL)
        ),
    CONSTRAINT uq_ask_stream_sessions__org_id_user UNIQUE (org_id, id, user_id)
);

CREATE INDEX idx_ask_stream_sessions__org_user_created
    ON ask_stream_sessions (org_id, user_id, created_at DESC);

CREATE INDEX idx_ask_stream_sessions__org_expires
    ON ask_stream_sessions (org_id, expires_at);

CREATE TABLE ask_stream_events (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL,
    session_id uuid NOT NULL,
    user_id uuid NOT NULL,
    sequence_no bigint NOT NULL CHECK (sequence_no >= 1),
    event_type text NOT NULL CHECK (event_type IN (
        'ask.started',
        'ask.warning',
        'ask.token',
        'ask.citations',
        'ask.version_context',
        'ask.completed',
        'stream.closed'
    )),
    envelope_version integer NOT NULL DEFAULT 1 CHECK (envelope_version >= 1),
    data jsonb NOT NULL DEFAULT '{}'::jsonb,
    payload_bytes integer NOT NULL CHECK (payload_bytes >= 0 AND payload_bytes <= 65536),
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_ask_stream_events__session_sequence
        UNIQUE (org_id, session_id, sequence_no),
    CONSTRAINT fk_ask_stream_events__session
        FOREIGN KEY (org_id, session_id, user_id)
        REFERENCES ask_stream_sessions (org_id, id, user_id)
        ON DELETE CASCADE
);

CREATE INDEX idx_ask_stream_events__session_sequence
    ON ask_stream_events (org_id, session_id, sequence_no);

ALTER TABLE ask_stream_sessions ENABLE ROW LEVEL SECURITY;
ALTER TABLE ask_stream_sessions FORCE ROW LEVEL SECURITY;
CREATE POLICY ask_stream_sessions_org_isolation ON ask_stream_sessions
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

ALTER TABLE ask_stream_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE ask_stream_events FORCE ROW LEVEL SECURITY;
CREATE POLICY ask_stream_events_org_isolation ON ask_stream_events
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());
