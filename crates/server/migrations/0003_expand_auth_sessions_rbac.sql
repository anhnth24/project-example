-- Phase: 1B
-- Owner: storage-owner, security-owner
-- Change: expand
-- Lock/data risk: additive columns/tables on empty POC auth surface; brief AccessExclusive on users for ADD COLUMN.
-- Rollback compatibility: no released application version depends on these tables; forward-only drop in a later contract migration if needed.
-- Auth sessions, invites, and canonical RBAC/group tables. Reuses orgs/users/org_memberships from 0001.

ALTER TABLE users
    ADD COLUMN password_hash text
        CHECK (password_hash IS NULL OR length(password_hash) >= 8);

CREATE TABLE permissions (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    code text NOT NULL,
    description text NOT NULL CHECK (length(trim(description)) > 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_permissions__code UNIQUE (code),
    CONSTRAINT ck_permissions__code CHECK (code ~ '^[a-z][a-z0-9_.]{1,63}$')
);

CREATE TABLE roles (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    code text NOT NULL,
    name text NOT NULL CHECK (length(trim(name)) > 0),
    is_system boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_roles__org_code UNIQUE (org_id, code),
    CONSTRAINT uq_roles__org_id_id UNIQUE (org_id, id),
    CONSTRAINT ck_roles__code CHECK (code ~ '^[a-z][a-z0-9_]{1,31}$')
);

CREATE INDEX idx_roles__org_id ON roles (org_id);

CREATE TABLE role_permissions (
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    role_id uuid NOT NULL,
    permission_id uuid NOT NULL REFERENCES permissions(id) ON DELETE RESTRICT,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (role_id, permission_id),
    CONSTRAINT fk_role_permissions__role_org
        FOREIGN KEY (org_id, role_id) REFERENCES roles(org_id, id) ON DELETE CASCADE
);

CREATE INDEX idx_role_permissions__org_id ON role_permissions (org_id);

CREATE TABLE groups (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    name text NOT NULL CHECK (length(trim(name)) > 0),
    description text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_groups__org_name UNIQUE (org_id, name),
    CONSTRAINT uq_groups__org_id_id UNIQUE (org_id, id)
);

CREATE INDEX idx_groups__org_id ON groups (org_id);

CREATE TABLE group_memberships (
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    group_id uuid NOT NULL,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (group_id, user_id),
    CONSTRAINT fk_group_memberships__group_org
        FOREIGN KEY (org_id, group_id) REFERENCES groups(org_id, id) ON DELETE CASCADE
);

CREATE INDEX idx_group_memberships__org_id ON group_memberships (org_id);
CREATE INDEX idx_group_memberships__org_user ON group_memberships (org_id, user_id);

CREATE TABLE refresh_tokens (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    family_id uuid NOT NULL,
    token_hash text NOT NULL,
    expires_at timestamptz NOT NULL,
    revoked_at timestamptz,
    replaced_by_id uuid REFERENCES refresh_tokens(id) ON DELETE SET NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_refresh_tokens__token_hash UNIQUE (token_hash),
    CONSTRAINT ck_refresh_tokens__hash_len CHECK (length(token_hash) >= 32),
    CONSTRAINT fk_refresh_tokens__membership
        FOREIGN KEY (org_id, user_id) REFERENCES org_memberships(org_id, user_id)
);

CREATE INDEX idx_refresh_tokens__org_user ON refresh_tokens (org_id, user_id);
CREATE INDEX idx_refresh_tokens__org_family ON refresh_tokens (org_id, family_id);

CREATE TABLE org_invites (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    email text NOT NULL CHECK (email = lower(email)),
    role text NOT NULL CHECK (role IN ('owner', 'admin', 'editor', 'viewer')),
    token_hash text NOT NULL,
    invited_by_user_id uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    expires_at timestamptz NOT NULL,
    accepted_at timestamptz,
    revoked_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_org_invites__token_hash UNIQUE (token_hash),
    CONSTRAINT ck_org_invites__hash_len CHECK (length(token_hash) >= 32),
    CONSTRAINT ck_org_invites__terminal_xor CHECK (
        NOT (accepted_at IS NOT NULL AND revoked_at IS NOT NULL)
    )
);

CREATE INDEX idx_org_invites__org_email ON org_invites (org_id, email);
