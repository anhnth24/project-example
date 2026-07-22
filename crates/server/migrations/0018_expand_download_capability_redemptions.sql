-- Phase: 1B
-- Owner: server-owner
-- Change: expand
-- Lock/data risk: additive table + RLS only.
-- Rollback compatibility: drop table download_capability_redemptions.
-- Single-use download capability JTI redemptions (P1B-R02).

CREATE TABLE download_capability_redemptions (
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    jti uuid NOT NULL,
    redeemed_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    PRIMARY KEY (org_id, jti),
    CONSTRAINT ck_download_capability_redemptions__expiry
        CHECK (expires_at > redeemed_at - interval '1 second')
);

CREATE INDEX idx_download_capability_redemptions__expires
    ON download_capability_redemptions (expires_at);

ALTER TABLE download_capability_redemptions ENABLE ROW LEVEL SECURITY;
ALTER TABLE download_capability_redemptions FORCE ROW LEVEL SECURITY;
CREATE POLICY download_capability_redemptions_org_isolation
    ON download_capability_redemptions
    USING (org_id = NULLIF(current_setting('app.org_id', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.org_id', true), '')::uuid);
