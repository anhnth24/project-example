-- Phase: 1B
-- Owner: storage-owner
-- Change: expand
-- Lock/data risk: creates empty collection ACL tables with composite FKs and non-concurrent indexes.
-- Rollback compatibility: additive only; no released readers depend on these tables yet.
-- Org-scoped collections with normalized principal access (real FKs, no polymorphic principal_id).

CREATE TABLE collections (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    name text NOT NULL CHECK (length(trim(name)) > 0),
    slug text NOT NULL CHECK (slug ~ '^[a-z0-9][a-z0-9-]{1,62}$'),
    description text,
    owner_user_id uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    visibility text NOT NULL DEFAULT 'private'
        CHECK (visibility IN ('private', 'org', 'groups')),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    deleted_at timestamptz,
    CONSTRAINT uq_collections__org_name UNIQUE (org_id, name),
    CONSTRAINT uq_collections__org_slug UNIQUE (org_id, slug),
    CONSTRAINT uq_collections__org_id_id UNIQUE (org_id, id)
);

CREATE INDEX idx_collections__org_id ON collections (org_id);
CREATE INDEX idx_collections__org_owner ON collections (org_id, owner_user_id);
CREATE INDEX idx_collections__org_slug ON collections (org_id, slug);

-- Normalized ACL: each principal type has durable composite FKs + ON DELETE CASCADE.
CREATE TABLE collection_user_access (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    collection_id uuid NOT NULL,
    user_id uuid NOT NULL,
    access_level text NOT NULL CHECK (access_level IN ('read', 'write', 'admin')),
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_collection_user_access__principal UNIQUE (collection_id, user_id),
    CONSTRAINT fk_collection_user_access__collection_org
        FOREIGN KEY (org_id, collection_id) REFERENCES collections(org_id, id) ON DELETE CASCADE,
    CONSTRAINT fk_collection_user_access__membership
        FOREIGN KEY (org_id, user_id) REFERENCES org_memberships(org_id, user_id) ON DELETE CASCADE
);

CREATE INDEX idx_collection_user_access__org_collection
    ON collection_user_access (org_id, collection_id);

CREATE TABLE collection_group_access (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    collection_id uuid NOT NULL,
    group_id uuid NOT NULL,
    access_level text NOT NULL CHECK (access_level IN ('read', 'write', 'admin')),
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_collection_group_access__principal UNIQUE (collection_id, group_id),
    CONSTRAINT fk_collection_group_access__collection_org
        FOREIGN KEY (org_id, collection_id) REFERENCES collections(org_id, id) ON DELETE CASCADE,
    CONSTRAINT fk_collection_group_access__group_org
        FOREIGN KEY (org_id, group_id) REFERENCES groups(org_id, id) ON DELETE CASCADE
);

CREATE INDEX idx_collection_group_access__org_collection
    ON collection_group_access (org_id, collection_id);

CREATE TABLE collection_role_access (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    collection_id uuid NOT NULL,
    role_id uuid NOT NULL,
    access_level text NOT NULL CHECK (access_level IN ('read', 'write', 'admin')),
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_collection_role_access__principal UNIQUE (collection_id, role_id),
    CONSTRAINT fk_collection_role_access__collection_org
        FOREIGN KEY (org_id, collection_id) REFERENCES collections(org_id, id) ON DELETE CASCADE,
    CONSTRAINT fk_collection_role_access__role_org
        FOREIGN KEY (org_id, role_id) REFERENCES roles(org_id, id) ON DELETE CASCADE
);

CREATE INDEX idx_collection_role_access__org_collection
    ON collection_role_access (org_id, collection_id);
