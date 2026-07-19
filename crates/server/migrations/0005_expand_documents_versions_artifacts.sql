-- Phase: 1B
-- Owner: storage-owner, retrieval-owner
-- Change: expand
-- Lock/data risk: creates empty document tables, publish helper, deferred invariant triggers, immutability.
-- Rollback compatibility: additive only; application rollback does not require dropping these tables.
-- Logical documents, immutable versions, DB-enforced current-published pointer, derived artifacts.
--
-- Immutability / publish mechanism (no caller-settable GUC):
--   * Content/identity columns: never UPDATE, never DELETE.
--   * publication_state: only draft→published (one-way).
--   * is_current: true→false anytime; false→true only when row is/becomes published.
--   * effective_to: only NULL→timestamp once (> effective_from); never rewritten.
--   * At-most-one current: partial unique index (RLS-immune).
--   * Deferred triggers: pointer/current agreement + fail-closed if app.org_id missing/mismatched.
--   * markhand_publish_document_version() is a convenience only — correctness does not depend on it.

CREATE TABLE documents (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    collection_id uuid NOT NULL,
    title text NOT NULL CHECK (length(trim(title)) > 0),
    state text NOT NULL DEFAULT 'uploaded'
        CHECK (state IN (
            'uploaded',
            'converting',
            'converted',
            'indexing',
            'indexed',
            'failed',
            'tombstoned',
            'purged'
        )),
    current_version_id uuid,
    created_by_user_id uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    deleted_at timestamptz,
    CONSTRAINT uq_documents__org_id_id UNIQUE (org_id, id),
    CONSTRAINT fk_documents__collection_org
        FOREIGN KEY (org_id, collection_id) REFERENCES collections(org_id, id) ON DELETE RESTRICT
);

CREATE INDEX idx_documents__org_collection ON documents (org_id, collection_id);
CREATE INDEX idx_documents__org_state ON documents (org_id, state);

CREATE TABLE document_versions (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    document_id uuid NOT NULL,
    version_number integer NOT NULL CHECK (version_number >= 1),
    parent_version_id uuid,
    publication_state text NOT NULL DEFAULT 'draft'
        CHECK (publication_state IN ('draft', 'published')),
    is_current boolean NOT NULL DEFAULT false,
    content_sha256 text NOT NULL CHECK (content_sha256 ~ '^[a-f0-9]{64}$'),
    original_object_key text NOT NULL CHECK (length(trim(original_object_key)) > 0),
    markdown_object_key text,
    source_filename text,
    source_content_type text,
    byte_size bigint CHECK (byte_size IS NULL OR byte_size >= 0),
    effective_from timestamptz NOT NULL DEFAULT now(),
    effective_to timestamptz,
    change_summary text,
    created_by_user_id uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_document_versions__org_id_id UNIQUE (org_id, id),
    CONSTRAINT uq_document_versions__org_document_id UNIQUE (org_id, document_id, id),
    CONSTRAINT uq_document_versions__org_document_number UNIQUE (org_id, document_id, version_number),
    CONSTRAINT ck_document_versions__effective_range CHECK (
        effective_to IS NULL OR effective_to > effective_from
    ),
    CONSTRAINT ck_document_versions__current_implies_published CHECK (
        NOT is_current OR publication_state = 'published'
    ),
    CONSTRAINT ck_document_versions__current_not_expired CHECK (
        NOT is_current OR effective_to IS NULL
    ),
    CONSTRAINT ck_document_versions__draft_not_current CHECK (
        publication_state <> 'draft' OR NOT is_current
    ),
    CONSTRAINT fk_document_versions__document_org
        FOREIGN KEY (org_id, document_id) REFERENCES documents(org_id, id) ON DELETE RESTRICT,
    CONSTRAINT fk_document_versions__parent_lineage
        FOREIGN KEY (org_id, document_id, parent_version_id)
        REFERENCES document_versions (org_id, document_id, id)
        DEFERRABLE INITIALLY IMMEDIATE
);

-- RLS-immune at-most-one current published version per logical document.
CREATE UNIQUE INDEX uq_document_versions__document_current
    ON document_versions (org_id, document_id)
    WHERE is_current;

CREATE INDEX idx_document_versions__org_document ON document_versions (org_id, document_id);
CREATE INDEX idx_document_versions__org_effective
    ON document_versions (org_id, document_id, effective_from, effective_to)
    WHERE publication_state = 'published';

ALTER TABLE documents
    ADD CONSTRAINT fk_documents__current_version_lineage
        FOREIGN KEY (org_id, id, current_version_id)
        REFERENCES document_versions (org_id, document_id, id)
        DEFERRABLE INITIALLY DEFERRED;

-- Legal state transitions only (no GUC authorization).
CREATE OR REPLACE FUNCTION document_versions_enforce_immutability()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'document_versions are immutable: DELETE is forbidden'
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;

    IF NEW.id IS DISTINCT FROM OLD.id
        OR NEW.org_id IS DISTINCT FROM OLD.org_id
        OR NEW.document_id IS DISTINCT FROM OLD.document_id
        OR NEW.version_number IS DISTINCT FROM OLD.version_number
        OR NEW.parent_version_id IS DISTINCT FROM OLD.parent_version_id
        OR NEW.content_sha256 IS DISTINCT FROM OLD.content_sha256
        OR NEW.original_object_key IS DISTINCT FROM OLD.original_object_key
        OR NEW.markdown_object_key IS DISTINCT FROM OLD.markdown_object_key
        OR NEW.source_filename IS DISTINCT FROM OLD.source_filename
        OR NEW.source_content_type IS DISTINCT FROM OLD.source_content_type
        OR NEW.byte_size IS DISTINCT FROM OLD.byte_size
        OR NEW.effective_from IS DISTINCT FROM OLD.effective_from
        OR NEW.change_summary IS DISTINCT FROM OLD.change_summary
        OR NEW.created_by_user_id IS DISTINCT FROM OLD.created_by_user_id
        OR NEW.created_at IS DISTINCT FROM OLD.created_at
    THEN
        RAISE EXCEPTION 'document_versions are immutable: content UPDATE is forbidden'
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;

    -- publication_state: only draft→published (one-way).
    IF NEW.publication_state IS DISTINCT FROM OLD.publication_state
        AND NOT (OLD.publication_state = 'draft' AND NEW.publication_state = 'published')
    THEN
        RAISE EXCEPTION 'document_versions: illegal publication_state transition % → %',
            OLD.publication_state, NEW.publication_state
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;

    -- is_current: true→false anytime; false→true only when published (after this update).
    IF NEW.is_current IS DISTINCT FROM OLD.is_current THEN
        IF OLD.is_current AND NOT NEW.is_current THEN
            NULL; -- supersede
        ELSIF NOT OLD.is_current AND NEW.is_current THEN
            IF NEW.publication_state <> 'published' THEN
                RAISE EXCEPTION 'document_versions: is_current may become true only when published'
                    USING ERRCODE = 'integrity_constraint_violation';
            END IF;
        END IF;
    END IF;

    -- effective_to: only NULL→timestamp once; never rewrite; must be > effective_from.
    IF NEW.effective_to IS DISTINCT FROM OLD.effective_to THEN
        IF OLD.effective_to IS NOT NULL THEN
            RAISE EXCEPTION 'document_versions: effective_to is immutable once set'
                USING ERRCODE = 'integrity_constraint_violation';
        END IF;
        IF NEW.effective_to IS NULL THEN
            RAISE EXCEPTION 'document_versions: effective_to cannot be cleared'
                USING ERRCODE = 'integrity_constraint_violation';
        END IF;
        IF NEW.effective_to <= NEW.effective_from THEN
            RAISE EXCEPTION 'document_versions: effective_to must be > effective_from'
                USING ERRCODE = 'integrity_constraint_violation';
        END IF;
        -- Cannot expire a still-current row (also enforced by CHECK).
        IF NEW.is_current THEN
            RAISE EXCEPTION 'document_versions: cannot set effective_to while is_current'
                USING ERRCODE = 'integrity_constraint_violation';
        END IF;
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER trg_document_versions__immutability
    BEFORE UPDATE OR DELETE ON document_versions
    FOR EACH ROW
    EXECUTE FUNCTION document_versions_enforce_immutability();

-- Deferred pointer/current agreement. Fail closed if tenant context missing/mismatched (RLS evasion).
-- At-most-one current is guaranteed by uq_document_versions__document_current (RLS-immune).
CREATE OR REPLACE FUNCTION markhand_validate_document_invariant(
    p_org_id uuid,
    p_document_id uuid
) RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
    ctx_org uuid;
    current_count integer;
    pointer uuid;
    current_id uuid;
    pub_state text;
    eff_from timestamptz;
    eff_to timestamptz;
    doc_state text;
    published_count integer;
BEGIN
    ctx_org := NULLIF(current_setting('app.org_id', true), '')::uuid;
    IF ctx_org IS NULL OR ctx_org IS DISTINCT FROM p_org_id THEN
        RAISE EXCEPTION
            'document invariant: app.org_id missing or mismatched (fail closed)'
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    SELECT d.current_version_id, d.state
    INTO pointer, doc_state
    FROM documents d
    WHERE d.org_id = p_org_id AND d.id = p_document_id;

    IF NOT FOUND THEN
        RAISE EXCEPTION
            'document invariant: document % not visible under app.org_id %',
            p_document_id, ctx_org
            USING ERRCODE = 'insufficient_privilege';
    END IF;

    SELECT count(*) FILTER (WHERE is_current)::integer,
           (array_agg(id) FILTER (WHERE is_current))[1],
           count(*) FILTER (WHERE publication_state = 'published')::integer
    INTO current_count, current_id, published_count
    FROM document_versions
    WHERE org_id = p_org_id AND document_id = p_document_id;

    -- current_count > 1 is also rejected by the partial unique index (primary RLS-immune guard).
    IF current_count > 1 THEN
        RAISE EXCEPTION
            'document invariant: more than one is_current for document %', p_document_id
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;

    IF published_count > 0 AND doc_state NOT IN ('tombstoned', 'purged') THEN
        IF current_count <> 1 THEN
            RAISE EXCEPTION
                'document invariant: published document % must have exactly one current version',
                p_document_id
                USING ERRCODE = 'integrity_constraint_violation';
        END IF;
    END IF;

    IF current_count = 0 THEN
        IF pointer IS NOT NULL THEN
            RAISE EXCEPTION
                'document invariant: current_version_id set without is_current row for %',
                p_document_id
                USING ERRCODE = 'integrity_constraint_violation';
        END IF;
        RETURN;
    END IF;

    IF pointer IS DISTINCT FROM current_id THEN
        RAISE EXCEPTION
            'document invariant: current_version_id % disagrees with is_current % for %',
            pointer, current_id, p_document_id
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;

    SELECT publication_state, effective_from, effective_to
    INTO pub_state, eff_from, eff_to
    FROM document_versions
    WHERE id = current_id;

    IF pub_state <> 'published' THEN
        RAISE EXCEPTION
            'document invariant: current version % is not published', current_id
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    IF eff_to IS NOT NULL THEN
        RAISE EXCEPTION
            'document invariant: current version % is expired (effective_to set)', current_id
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    IF eff_from > clock_timestamp() THEN
        RAISE EXCEPTION
            'document invariant: current version % is future-dated', current_id
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
END;
$$;

CREATE OR REPLACE FUNCTION markhand_validate_document_invariant_on_version()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM markhand_validate_document_invariant(NEW.org_id, NEW.document_id);
    RETURN NULL;
END;
$$;

CREATE OR REPLACE FUNCTION markhand_validate_document_invariant_on_document()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM markhand_validate_document_invariant(NEW.org_id, NEW.id);
    RETURN NULL;
END;
$$;

CREATE CONSTRAINT TRIGGER trg_document_versions__validate_invariants
    AFTER INSERT OR UPDATE ON document_versions
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION markhand_validate_document_invariant_on_version();

CREATE CONSTRAINT TRIGGER trg_documents__validate_invariants
    AFTER UPDATE OF current_version_id, state ON documents
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION markhand_validate_document_invariant_on_document();

-- Convenience publish/supersede using only legal transitions (no GUC).
CREATE OR REPLACE FUNCTION markhand_publish_document_version(
    p_org_id uuid,
    p_document_id uuid,
    p_version_id uuid,
    p_effective_at timestamptz DEFAULT clock_timestamp()
) RETURNS uuid
LANGUAGE plpgsql
AS $$
DECLARE
    v_state text;
    v_is_current boolean;
    v_eff_from timestamptz;
BEGIN
    PERFORM 1 FROM documents
    WHERE org_id = p_org_id AND id = p_document_id
    FOR UPDATE;

    SELECT publication_state, is_current, effective_from
    INTO v_state, v_is_current, v_eff_from
    FROM document_versions
    WHERE org_id = p_org_id AND document_id = p_document_id AND id = p_version_id
    FOR UPDATE;

    IF NOT FOUND THEN
        RAISE EXCEPTION 'version % not found for document %', p_version_id, p_document_id
            USING ERRCODE = 'no_data_found';
    END IF;

    IF v_is_current THEN
        UPDATE documents
        SET current_version_id = p_version_id, updated_at = clock_timestamp()
        WHERE org_id = p_org_id AND id = p_document_id;
        RETURN p_version_id;
    END IF;

    IF v_state = 'published' THEN
        RAISE EXCEPTION 'version % is already published and not current; create a new version', p_version_id
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;

    IF p_effective_at > clock_timestamp() THEN
        RAISE EXCEPTION 'cannot publish a future-dated current version'
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;

    IF v_eff_from > p_effective_at THEN
        RAISE EXCEPTION 'version effective_from % is after publish time %', v_eff_from, p_effective_at
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;

    -- Supersede prior current (legal: is_current true→false + effective_to NULL→ts).
    UPDATE document_versions
    SET is_current = false,
        effective_to = p_effective_at
    WHERE org_id = p_org_id
      AND document_id = p_document_id
      AND is_current
      AND effective_to IS NULL;

    -- Promote draft → published current (legal: draft→published + false→true).
    UPDATE document_versions
    SET publication_state = 'published',
        is_current = true
    WHERE id = p_version_id;

    UPDATE documents
    SET current_version_id = p_version_id,
        updated_at = clock_timestamp()
    WHERE org_id = p_org_id AND id = p_document_id;

    RETURN p_version_id;
END;
$$;

CREATE TABLE derived_artifacts (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    document_id uuid NOT NULL,
    version_id uuid NOT NULL,
    artifact_kind text NOT NULL CHECK (artifact_kind IN (
        'markdown',
        'preview',
        'thumbnail',
        'extracted_text',
        'other'
    )),
    object_key text NOT NULL CHECK (length(trim(object_key)) > 0),
    content_sha256 text NOT NULL CHECK (content_sha256 ~ '^[a-f0-9]{64}$'),
    content_type text,
    byte_size bigint CHECK (byte_size IS NULL OR byte_size >= 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_derived_artifacts__version_kind UNIQUE (version_id, artifact_kind),
    CONSTRAINT fk_derived_artifacts__version_lineage
        FOREIGN KEY (org_id, document_id, version_id)
        REFERENCES document_versions (org_id, document_id, id) ON DELETE RESTRICT
);

CREATE INDEX idx_derived_artifacts__org_version ON derived_artifacts (org_id, version_id);

CREATE OR REPLACE FUNCTION derived_artifacts_enforce_immutability()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION 'derived_artifacts are immutable: % is forbidden', TG_OP
        USING ERRCODE = 'integrity_constraint_violation';
END;
$$;

CREATE TRIGGER trg_derived_artifacts__immutability
    BEFORE UPDATE OR DELETE ON derived_artifacts
    FOR EACH ROW
    EXECUTE FUNCTION derived_artifacts_enforce_immutability();
