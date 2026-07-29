-- Phase: 2
-- Owner: storage-owner
-- Change: expand
-- Lock/data risk: creates one new, empty org-scoped table (`projects`) and
--   adds one nullable FK column to `collections` (`ADD COLUMN ... DEFAULT
--   NULL` is a metadata-only change on the PostgreSQL versions this project
--   targets — no table rewrite, no long lock — even though `collections`
--   already holds POC rows). The two new indexes and the new composite FK on
--   `collections` are built against the existing (small, POC-scale) table.
-- Rollback compatibility: additive only. No released application version
--   reads or writes `projects` or `collections.project_id` before this
--   migration ships; dropping both later would be compatible with any
--   released client.
--
-- P2-18 (owner request 2026-07-29): `org -> project -> collection ->
-- document` grouping so Q&A and the Library can be scoped to one project or
-- to everything. A project is a named, org-scoped folder of collections; a
-- collection belongs to at most one project via the new nullable
-- `collections.project_id` — an unassigned collection (`project_id IS NULL`)
-- keeps working exactly as it does today, byte-for-byte (every existing
-- collection query that does not know about `project_id` is unaffected).
-- "All projects" in the API/UI is simply "no project filter" — it is never a
-- real `projects` row, so there is no sentinel id to reserve or migrate.
--
-- Same org-scoped RLS shape `collections` itself uses (migrations/0004 +
-- 0010): `org_id` FK to `orgs`, `FORCE ROW LEVEL SECURITY`, and a
-- `(org_id, id)` unique constraint so `collections` can carry a composite FK
-- into `projects` that keeps `project_id` org-consistent even if RLS were
-- ever bypassed (same pattern `collection_user_access` etc. already use
-- against `collections(org_id, id)`).

CREATE TABLE projects (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    name text NOT NULL CHECK (length(trim(name)) > 0 AND length(name) <= 200),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_projects__org_name UNIQUE (org_id, name),
    CONSTRAINT uq_projects__org_id_id UNIQUE (org_id, id)
);

CREATE INDEX idx_projects__org_id ON projects (org_id);

ALTER TABLE projects ENABLE ROW LEVEL SECURITY;
ALTER TABLE projects FORCE ROW LEVEL SECURITY;
CREATE POLICY projects_org_isolation ON projects
    USING (org_id = markhand_current_org_id())
    WITH CHECK (org_id = markhand_current_org_id());

-- Nullable: a collection with no project assigned behaves exactly as before
-- this migration (1 collection belongs to at most 1 project — enforced by
-- this being a plain scalar FK column, not a join table).
ALTER TABLE collections
    ADD COLUMN project_id uuid REFERENCES projects (id) ON DELETE SET NULL;

-- Composite FK (MATCH SIMPLE — the PostgreSQL default): satisfied
-- automatically whenever project_id IS NULL, and otherwise requires the
-- referenced projects row to share the same org_id. This is what actually
-- prevents a collection from ever pointing at another org's project.
ALTER TABLE collections
    ADD CONSTRAINT fk_collections__project_org
        FOREIGN KEY (org_id, project_id) REFERENCES projects (org_id, id)
        ON DELETE SET NULL;

CREATE INDEX idx_collections__org_project ON collections (org_id, project_id);
