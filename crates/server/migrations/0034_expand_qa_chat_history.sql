-- Phase: 2
-- Owner: api-owner
-- Change: expand
-- Lock/data risk: creates two new, empty org-scoped tables
--   (`qa_chat_sessions`, `qa_chat_turns`) — no existing table touched, no
--   lock risk.
-- Rollback compatibility: additive only. No released application version
--   reads or writes either table before this migration ships; dropping both
--   later would be compatible with any released client.
--
-- P2-19 (owner request 2026-07-29): private per-user chat history for the
-- Q&A surface. A chat session belongs to exactly one (org, user) — there is
-- no "shared/team" chat history in this slice, and no endpoint ever lets a
-- caller read, list, or mutate another user's session, even within the same
-- org (see `db::chat_sessions` / `routes::chat_sessions`). RLS below is
-- org-scoped only (same shape every other org-scoped table uses); the
-- per-user boundary is an *additional* `user_id = caller` predicate every
-- query applies on top of it — same "RLS is org isolation, ownership is an
-- extra WHERE clause" pattern `ask_stream_sessions` (migrations/0024)
-- already established for per-user-owned rows, including reusing its
-- `(org_id, id, user_id)` composite unique constraint for the identical
-- reason: it lets `qa_chat_turns` carry a composite FK back to
-- `qa_chat_sessions` that stays user-consistent even if RLS were ever
-- bypassed.
--
-- Citations/warnings are stored as opaque `jsonb` — the exact pins/strings
-- the client already displayed after its own SSE stream (or JSON /ask)
-- completed — and are never re-validated when the session is read back; the
-- client re-validates on click-through via POST /citations/resolve, same as
-- any other citation deep-link. See `routes::chat_sessions`'s append/get
-- doc comments for that trade-off.

CREATE TABLE qa_chat_sessions (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    title text NOT NULL CHECK (length(trim(title)) > 0 AND length(title) <= 200),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_qa_chat_sessions__org_id_user UNIQUE (org_id, id, user_id)
);

-- Newest-first per-caller listing (GET /chat-sessions): appending a turn
-- bumps `updated_at` so "recent chats" surfaces active conversations first.
CREATE INDEX idx_qa_chat_sessions__org_user_updated
    ON qa_chat_sessions (org_id, user_id, updated_at DESC, id DESC);

ALTER TABLE qa_chat_sessions ENABLE ROW LEVEL SECURITY;
ALTER TABLE qa_chat_sessions FORCE ROW LEVEL SECURITY;
CREATE POLICY qa_chat_sessions_org_isolation ON qa_chat_sessions
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

CREATE TABLE qa_chat_turns (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL,
    session_id uuid NOT NULL,
    user_id uuid NOT NULL,
    seq integer NOT NULL CHECK (seq >= 1),
    -- Bounded the same way ask.rs/search.rs already bound an inbound
    -- question (8_192 chars) — this is the server-side record of a question
    -- the client already got an answer for, not a fresh validation surface.
    question text NOT NULL
        CHECK (length(trim(question)) > 0 AND length(question) <= 8192),
    -- Unbounded like `chunks.body`: a grounded answer's length is bounded by
    -- retrieval/provider behavior upstream, not an arbitrary column CHECK.
    answer text NOT NULL,
    answer_mode text NOT NULL CHECK (answer_mode IN (
        'offline_extractive', 'fallback_extractive', 'local_llm', 'cloud_llm',
        'subscription_cli', 'llm_unverified'
    )),
    citations jsonb NOT NULL DEFAULT '[]'::jsonb,
    warnings jsonb NOT NULL DEFAULT '[]'::jsonb,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_qa_chat_turns__session_seq UNIQUE (session_id, seq),
    CONSTRAINT fk_qa_chat_turns__session
        FOREIGN KEY (org_id, session_id, user_id)
        REFERENCES qa_chat_sessions (org_id, id, user_id)
        ON DELETE CASCADE
);

CREATE INDEX idx_qa_chat_turns__session_seq
    ON qa_chat_turns (org_id, session_id, seq);

ALTER TABLE qa_chat_turns ENABLE ROW LEVEL SECURITY;
ALTER TABLE qa_chat_turns FORCE ROW LEVEL SECURITY;
CREATE POLICY qa_chat_turns_org_isolation ON qa_chat_turns
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());
