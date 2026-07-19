-- Phase: 1B
-- Owner: storage-owner, security-owner
-- Change: expand
-- Lock/data risk: ENABLE+FORCE RLS and policy creation on empty tenant tables; brief AccessExclusive per table.
-- Rollback compatibility: policies can be dropped in a later contract migration; application must keep SET LOCAL app.org_id.
-- Tenant isolation RLS for shared business tables (ADR 0007), matching 0002 org_memberships pattern.

CREATE OR REPLACE FUNCTION markhand_current_org_id()
RETURNS uuid
LANGUAGE sql
STABLE
AS $$
    SELECT NULLIF(current_setting('app.org_id', true), '')::uuid;
$$;

ALTER TABLE roles ENABLE ROW LEVEL SECURITY;
ALTER TABLE roles FORCE ROW LEVEL SECURITY;
CREATE POLICY roles_org_isolation ON roles
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

ALTER TABLE role_permissions ENABLE ROW LEVEL SECURITY;
ALTER TABLE role_permissions FORCE ROW LEVEL SECURITY;
CREATE POLICY role_permissions_org_isolation ON role_permissions
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

ALTER TABLE groups ENABLE ROW LEVEL SECURITY;
ALTER TABLE groups FORCE ROW LEVEL SECURITY;
CREATE POLICY groups_org_isolation ON groups
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

ALTER TABLE group_memberships ENABLE ROW LEVEL SECURITY;
ALTER TABLE group_memberships FORCE ROW LEVEL SECURITY;
CREATE POLICY group_memberships_org_isolation ON group_memberships
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

ALTER TABLE refresh_tokens ENABLE ROW LEVEL SECURITY;
ALTER TABLE refresh_tokens FORCE ROW LEVEL SECURITY;
CREATE POLICY refresh_tokens_org_isolation ON refresh_tokens
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

ALTER TABLE org_invites ENABLE ROW LEVEL SECURITY;
ALTER TABLE org_invites FORCE ROW LEVEL SECURITY;
CREATE POLICY org_invites_org_isolation ON org_invites
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

ALTER TABLE collections ENABLE ROW LEVEL SECURITY;
ALTER TABLE collections FORCE ROW LEVEL SECURITY;
CREATE POLICY collections_org_isolation ON collections
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

ALTER TABLE collection_user_access ENABLE ROW LEVEL SECURITY;
ALTER TABLE collection_user_access FORCE ROW LEVEL SECURITY;
CREATE POLICY collection_user_access_org_isolation ON collection_user_access
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

ALTER TABLE collection_group_access ENABLE ROW LEVEL SECURITY;
ALTER TABLE collection_group_access FORCE ROW LEVEL SECURITY;
CREATE POLICY collection_group_access_org_isolation ON collection_group_access
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

ALTER TABLE collection_role_access ENABLE ROW LEVEL SECURITY;
ALTER TABLE collection_role_access FORCE ROW LEVEL SECURITY;
CREATE POLICY collection_role_access_org_isolation ON collection_role_access
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

ALTER TABLE documents ENABLE ROW LEVEL SECURITY;
ALTER TABLE documents FORCE ROW LEVEL SECURITY;
CREATE POLICY documents_org_isolation ON documents
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

ALTER TABLE document_versions ENABLE ROW LEVEL SECURITY;
ALTER TABLE document_versions FORCE ROW LEVEL SECURITY;
CREATE POLICY document_versions_org_isolation ON document_versions
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

ALTER TABLE derived_artifacts ENABLE ROW LEVEL SECURITY;
ALTER TABLE derived_artifacts FORCE ROW LEVEL SECURITY;
CREATE POLICY derived_artifacts_org_isolation ON derived_artifacts
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

ALTER TABLE chunks ENABLE ROW LEVEL SECURITY;
ALTER TABLE chunks FORCE ROW LEVEL SECURITY;
CREATE POLICY chunks_org_isolation ON chunks
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

ALTER TABLE claims ENABLE ROW LEVEL SECURITY;
ALTER TABLE claims FORCE ROW LEVEL SECURITY;
CREATE POLICY claims_org_isolation ON claims
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

ALTER TABLE conflicts ENABLE ROW LEVEL SECURITY;
ALTER TABLE conflicts FORCE ROW LEVEL SECURITY;
CREATE POLICY conflicts_org_isolation ON conflicts
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

ALTER TABLE conflict_evidence ENABLE ROW LEVEL SECURITY;
ALTER TABLE conflict_evidence FORCE ROW LEVEL SECURITY;
CREATE POLICY conflict_evidence_org_isolation ON conflict_evidence
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

ALTER TABLE jobs ENABLE ROW LEVEL SECURITY;
ALTER TABLE jobs FORCE ROW LEVEL SECURITY;
CREATE POLICY jobs_org_isolation ON jobs
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

ALTER TABLE outbox_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE outbox_events FORCE ROW LEVEL SECURITY;
CREATE POLICY outbox_events_org_isolation ON outbox_events
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

ALTER TABLE event_log ENABLE ROW LEVEL SECURITY;
ALTER TABLE event_log FORCE ROW LEVEL SECURITY;
CREATE POLICY event_log_org_isolation ON event_log
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

ALTER TABLE org_quotas ENABLE ROW LEVEL SECURITY;
ALTER TABLE org_quotas FORCE ROW LEVEL SECURITY;
CREATE POLICY org_quotas_org_isolation ON org_quotas
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

ALTER TABLE usage_counters ENABLE ROW LEVEL SECURITY;
ALTER TABLE usage_counters FORCE ROW LEVEL SECURITY;
CREATE POLICY usage_counters_org_isolation ON usage_counters
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

ALTER TABLE quota_reservations ENABLE ROW LEVEL SECURITY;
ALTER TABLE quota_reservations FORCE ROW LEVEL SECURITY;
CREATE POLICY quota_reservations_org_isolation ON quota_reservations
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

ALTER TABLE audit_log ENABLE ROW LEVEL SECURITY;
ALTER TABLE audit_log FORCE ROW LEVEL SECURITY;
CREATE POLICY audit_log_org_isolation ON audit_log
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

ALTER TABLE index_metadata ENABLE ROW LEVEL SECURITY;
ALTER TABLE index_metadata FORCE ROW LEVEL SECURITY;
CREATE POLICY index_metadata_org_isolation ON index_metadata
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());
