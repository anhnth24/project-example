-- Phase: 1B
-- Owner: storage-owner, security-owner
-- Change: expand
-- Lock/data risk: brief ACCESS EXCLUSIVE lock on the empty POC membership table.
-- Rollback compatibility: application does not access memberships until OrgContext routes exist.
ALTER TABLE org_memberships ENABLE ROW LEVEL SECURITY;
ALTER TABLE org_memberships FORCE ROW LEVEL SECURITY;

CREATE POLICY org_memberships_org_isolation ON org_memberships
    USING (org_id = NULLIF(current_setting('app.org_id', true), '')::uuid)
    WITH CHECK (org_id = NULLIF(current_setting('app.org_id', true), '')::uuid);
