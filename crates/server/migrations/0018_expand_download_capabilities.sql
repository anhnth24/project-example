-- Phase: 1B
-- Owner: retrieval-owner, security-owner
-- Change: expand
-- Lock/data risk: creates empty download_capabilities table + RLS policy.
-- Rollback compatibility: additive; drop table to reverse.
--
-- Single-use, short-lived download capabilities (P1B-R02 / ADR 0002 / ADR 0007).
-- Tokens are HMAC-bound to org/user/document/version/purpose/hash/type/size;
-- PostgreSQL is the authority for issuance, expiry and replay (consumed_at).
-- Raw MinIO credentials and object keys are never returned to clients.

CREATE TABLE download_capabilities (
    id uuid PRIMARY KEY,
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    document_id uuid NOT NULL,
    version_id uuid NOT NULL,
    purpose text NOT NULL
        CHECK (purpose IN ('original', 'markdown')),
    content_sha256 text NOT NULL
        CHECK (content_sha256 ~ '^[0-9a-f]{64}$'),
    content_type text NOT NULL
        CHECK (length(trim(content_type)) > 0 AND length(content_type) <= 255),
    byte_size bigint NOT NULL
        CHECK (byte_size > 0),
    expires_at timestamptz NOT NULL,
    consumed_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT fk_download_capabilities__document_org
        FOREIGN KEY (org_id, document_id) REFERENCES documents (org_id, id)
        ON DELETE CASCADE,
    CONSTRAINT fk_download_capabilities__version_org
        FOREIGN KEY (org_id, document_id, version_id)
        REFERENCES document_versions (org_id, document_id, id)
        ON DELETE CASCADE,
    CONSTRAINT ck_download_capabilities__expires_after_created
        CHECK (expires_at > created_at)
);

CREATE INDEX idx_download_capabilities__org_user_open
    ON download_capabilities (org_id, user_id, expires_at)
    WHERE consumed_at IS NULL;

CREATE INDEX idx_download_capabilities__org_id
    ON download_capabilities (org_id, id);

ALTER TABLE download_capabilities ENABLE ROW LEVEL SECURITY;
ALTER TABLE download_capabilities FORCE ROW LEVEL SECURITY;
CREATE POLICY download_capabilities_org_isolation ON download_capabilities
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());
