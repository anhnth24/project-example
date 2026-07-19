-- Phase: 1B
-- Owner: storage-owner
-- Change: expand
-- Lock/data risk: inserts synthetic POC seed rows only; no locks on hot production paths.
-- Rollback compatibility: seed is additive and clearly separable from schema; delete by fixed UUIDs if needed.
-- POC-only seed: one org, four built-in roles (owner/admin/editor/viewer), permissions, admin user.
-- NO password_hash / secrets here — F05 owns auth credential material. Inserts are idempotent-safe.

INSERT INTO orgs (id, slug, name)
VALUES (
    '11111111-1111-1111-1111-111111111111',
    'poc',
    'Markhand POC'
)
ON CONFLICT (id) DO NOTHING;

-- RLS is FORCE'd on tenant tables; seed must set transaction-local org context.
-- apply_migrations wraps each file in a transaction, so SET LOCAL applies to all seed DML.
SET LOCAL app.org_id = '11111111-1111-1111-1111-111111111111';

-- User row only — no password/secret columns populated (F05).
INSERT INTO users (id, email, display_name)
VALUES (
    '22222222-2222-2222-2222-222222222201',
    'admin@poc.example',
    'POC Admin'
)
ON CONFLICT (id) DO NOTHING;

INSERT INTO org_memberships (org_id, user_id, role)
VALUES (
    '11111111-1111-1111-1111-111111111111',
    '22222222-2222-2222-2222-222222222201',
    'admin'
)
ON CONFLICT (org_id, user_id) DO NOTHING;

INSERT INTO permissions (id, code, description)
VALUES
    ('33333333-3333-3333-3333-333333333301', 'doc.upload', 'Upload documents'),
    ('33333333-3333-3333-3333-333333333302', 'doc.delete', 'Delete documents'),
    ('33333333-3333-3333-3333-333333333303', 'doc.publish', 'Publish document versions'),
    ('33333333-3333-3333-3333-333333333304', 'qa.query', 'Run search and Q&A'),
    ('33333333-3333-3333-3333-333333333305', 'member.manage', 'Manage org membership'),
    ('33333333-3333-3333-3333-333333333306', 'audit.view', 'View audit log')
ON CONFLICT (id) DO NOTHING;

-- Four built-in roles matching org_memberships.role CHECK (owner/admin/editor/viewer).
INSERT INTO roles (id, org_id, code, name, is_system)
VALUES
    ('44444444-4444-4444-4444-444444444401', '11111111-1111-1111-1111-111111111111', 'owner', 'Owner', true),
    ('44444444-4444-4444-4444-444444444402', '11111111-1111-1111-1111-111111111111', 'admin', 'Admin', true),
    ('44444444-4444-4444-4444-444444444403', '11111111-1111-1111-1111-111111111111', 'editor', 'Editor', true),
    ('44444444-4444-4444-4444-444444444404', '11111111-1111-1111-1111-111111111111', 'viewer', 'Viewer', true)
ON CONFLICT (id) DO NOTHING;

-- Owner + admin get full POC permission set; editor/viewer get narrower subsets.
INSERT INTO role_permissions (org_id, role_id, permission_id)
SELECT '11111111-1111-1111-1111-111111111111', role_id, permission_id
FROM (
    VALUES
        ('44444444-4444-4444-4444-444444444401'::uuid),
        ('44444444-4444-4444-4444-444444444402'::uuid)
) AS roles(role_id)
CROSS JOIN (
    SELECT id AS permission_id FROM permissions
) AS perms
ON CONFLICT (role_id, permission_id) DO NOTHING;

INSERT INTO role_permissions (org_id, role_id, permission_id)
VALUES
    ('11111111-1111-1111-1111-111111111111', '44444444-4444-4444-4444-444444444403', '33333333-3333-3333-3333-333333333301'),
    ('11111111-1111-1111-1111-111111111111', '44444444-4444-4444-4444-444444444403', '33333333-3333-3333-3333-333333333303'),
    ('11111111-1111-1111-1111-111111111111', '44444444-4444-4444-4444-444444444403', '33333333-3333-3333-3333-333333333304'),
    ('11111111-1111-1111-1111-111111111111', '44444444-4444-4444-4444-444444444404', '33333333-3333-3333-3333-333333333304')
ON CONFLICT (role_id, permission_id) DO NOTHING;

INSERT INTO collections (id, org_id, name, slug, description, owner_user_id, visibility)
VALUES (
    '55555555-5555-5555-5555-555555555501',
    '11111111-1111-1111-1111-111111111111',
    'POC Library',
    'poc-library',
    'Synthetic collection for the single-org POC',
    '22222222-2222-2222-2222-222222222201',
    'org'
)
ON CONFLICT (id) DO NOTHING;

INSERT INTO org_quotas (
    org_id,
    max_storage_bytes,
    max_documents,
    max_concurrent_jobs,
    max_monthly_tokens
)
VALUES (
    '11111111-1111-1111-1111-111111111111',
    10737418240,
    10000,
    4,
    5000000
)
ON CONFLICT (org_id) DO NOTHING;
