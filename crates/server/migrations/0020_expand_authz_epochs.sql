-- Phase: 1B
-- Owner: retrieval-owner
-- Change: expand
-- Lock/data risk: creates empty authz epoch tables + bump helpers for Q&A stream fencing (R03/R05).
-- Rollback compatibility: schema only; readers tolerate missing rows (default epoch 1).
-- Cross-process authorization epochs for membership/ACL/document mutation fences.

CREATE TABLE authz_epochs (
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    epoch bigint NOT NULL DEFAULT 1 CHECK (epoch >= 1),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, user_id)
);

CREATE TABLE document_authz_epochs (
    org_id uuid NOT NULL,
    document_id uuid NOT NULL,
    epoch bigint NOT NULL DEFAULT 1 CHECK (epoch >= 1),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, document_id),
    CONSTRAINT fk_document_authz_epochs__document
        FOREIGN KEY (org_id, document_id) REFERENCES documents (org_id, id)
        ON DELETE CASCADE
);

CREATE INDEX idx_authz_epochs__updated ON authz_epochs (updated_at);
CREATE INDEX idx_document_authz_epochs__updated ON document_authz_epochs (updated_at);

ALTER TABLE authz_epochs ENABLE ROW LEVEL SECURITY;
ALTER TABLE authz_epochs FORCE ROW LEVEL SECURITY;
ALTER TABLE document_authz_epochs ENABLE ROW LEVEL SECURITY;
ALTER TABLE document_authz_epochs FORCE ROW LEVEL SECURITY;

CREATE POLICY authz_epochs_tenant ON authz_epochs
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

CREATE POLICY document_authz_epochs_tenant ON document_authz_epochs
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());
