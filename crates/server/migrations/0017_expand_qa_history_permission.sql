-- Phase: 1B
-- Owner: retrieval-owner
-- Change: expand
-- Lock/data risk: additive permission and POC role grants only.
-- Rollback compatibility: remove the fixed role grants and permission row.
-- ADR 0002 requires explicit authorization before superseded versions are exposed.

INSERT INTO permissions (id, code, description)
VALUES (
    '33333333-3333-3333-3333-333333333307',
    'qa.history',
    'Query superseded document versions'
)
ON CONFLICT (id) DO NOTHING;

SET LOCAL app.org_id = '11111111-1111-1111-1111-111111111111';

-- History is deliberately narrower than qa.query in the single-org POC.
-- Owner/admin may inspect prior versions; editor/viewer remain current-only.
INSERT INTO role_permissions (org_id, role_id, permission_id)
VALUES
    (
        '11111111-1111-1111-1111-111111111111',
        '44444444-4444-4444-4444-444444444401',
        '33333333-3333-3333-3333-333333333307'
    ),
    (
        '11111111-1111-1111-1111-111111111111',
        '44444444-4444-4444-4444-444444444402',
        '33333333-3333-3333-3333-333333333307'
    )
ON CONFLICT (role_id, permission_id) DO NOTHING;
