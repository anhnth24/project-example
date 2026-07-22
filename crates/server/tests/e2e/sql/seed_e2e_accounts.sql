-- P1B-O04 test-only accounts (synthetic emails). Requires app.org_id GUC for RLS.
-- Invoked by deploy/scripts/seed-poc-e2e.sh — never against human environments.

BEGIN;
SELECT set_config('app.org_id', '11111111-1111-1111-1111-111111111111', true);

INSERT INTO users (id, email, display_name)
VALUES
  ('22222222-2222-2222-2222-222222222211', 'editor-e2e@poc.example', 'E2E Editor'),
  ('22222222-2222-2222-2222-222222222212', 'viewer-e2e@poc.example', 'E2E Viewer')
ON CONFLICT (id) DO UPDATE SET email = EXCLUDED.email, display_name = EXCLUDED.display_name;

INSERT INTO org_memberships (org_id, user_id, role)
VALUES
  ('11111111-1111-1111-1111-111111111111', '22222222-2222-2222-2222-222222222211', 'editor'),
  ('11111111-1111-1111-1111-111111111111', '22222222-2222-2222-2222-222222222212', 'viewer')
ON CONFLICT (org_id, user_id) DO UPDATE SET role = EXCLUDED.role;

-- Second org for IDOR matrix.
INSERT INTO orgs (id, slug, name)
VALUES ('12121212-1212-4212-8212-121212121212', 'poc-e2e-b', 'Markhand E2E Org B')
ON CONFLICT (id) DO NOTHING;

SELECT set_config('app.org_id', '12121212-1212-4212-8212-121212121212', true);

INSERT INTO users (id, email, display_name)
VALUES ('23232323-2323-4232-8232-232323232301', 'owner@org-b.example', 'E2E OrgB Owner')
ON CONFLICT (id) DO UPDATE SET email = EXCLUDED.email;

INSERT INTO org_memberships (org_id, user_id, role)
VALUES ('12121212-1212-4212-8212-121212121212', '23232323-2323-4232-8232-232323232301', 'owner')
ON CONFLICT (org_id, user_id) DO UPDATE SET role = EXCLUDED.role;

INSERT INTO roles (id, org_id, code, name, is_system)
VALUES
  ('45454545-4545-4545-8545-454545454501', '12121212-1212-4212-8212-121212121212', 'owner', 'Owner', true)
ON CONFLICT (id) DO NOTHING;

INSERT INTO role_permissions (org_id, role_id, permission_id)
SELECT '12121212-1212-4212-8212-121212121212', '45454545-4545-4545-8545-454545454501', p.id
FROM permissions p
ON CONFLICT (role_id, permission_id) DO NOTHING;

INSERT INTO collections (id, org_id, name, slug, description, owner_user_id, visibility)
VALUES (
  ('56565656-5656-4565-8565-565656565601'),
  '12121212-1212-4212-8212-121212121212',
  'Org B Library',
  'org-b-library',
  'Synthetic foreign collection',
  '23232323-2323-4232-8232-232323232301',
  'org'
)
ON CONFLICT (id) DO NOTHING;

INSERT INTO org_quotas (org_id, max_storage_bytes, max_documents, max_concurrent_jobs, max_monthly_tokens)
VALUES ('12121212-1212-4212-8212-121212121212', 1073741824, 1000, 2, 100000)
ON CONFLICT (org_id) DO NOTHING;

-- Foreign document/version IDs for IDOR matrix (org B only).
INSERT INTO documents (
  id, org_id, collection_id, title, state, created_by_user_id
) VALUES (
  '67676767-6767-4676-8676-676767676701',
  '12121212-1212-4212-8212-121212121212',
  '56565656-5656-4565-8565-565656565601',
  'E2E Foreign IDOR Document',
  'indexed',
  '23232323-2323-4232-8232-232323232301'
)
ON CONFLICT (id) DO NOTHING;

INSERT INTO document_versions (
  id, org_id, document_id, version_number, publication_state, is_current,
  content_sha256, original_object_key, source_filename, source_content_type,
  byte_size, created_by_user_id
) VALUES (
  '68686868-6868-4686-8686-686868686801',
  '12121212-1212-4212-8212-121212121212',
  '67676767-6767-4676-8676-676767676701',
  1,
  'published',
  true,
  'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
  'e2e/foreign/idor-seed.txt',
  'idor-seed.txt',
  'text/plain',
  32,
  '23232323-2323-4232-8232-232323232301'
)
ON CONFLICT (id) DO NOTHING;

UPDATE documents
SET current_version_id = '68686868-6868-4686-8686-686868686801'
WHERE id = '67676767-6767-4676-8676-676767676701'
  AND current_version_id IS NULL;

COMMIT;