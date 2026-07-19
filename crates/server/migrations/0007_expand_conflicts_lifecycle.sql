-- Phase: 1B
-- Owner: storage-owner, retrieval-owner
-- Change: expand
-- Lock/data risk: creates empty conflict lifecycle tables, immutability/transition triggers, indexes.
-- Rollback compatibility: additive only; conflict history is append-friendly for later phases.
-- Cross-document conflict + immutable evidence lifecycle per ADR 0003.

CREATE TABLE conflicts (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    status text NOT NULL DEFAULT 'open'
        CHECK (status IN ('open', 'resolved', 'accepted_exception', 'false_positive')),
    severity text NOT NULL DEFAULT 'warning'
        CHECK (severity IN ('info', 'warning', 'high')),
    conflict_type text NOT NULL CHECK (conflict_type IN (
        'numeric',
        'enum',
        'date',
        'limit',
        'must_vs_must_not',
        'other'
    )),
    claim_a_id uuid NOT NULL,
    claim_b_id uuid NOT NULL,
    first_detected_at timestamptz NOT NULL DEFAULT now(),
    first_detected_version_id uuid,
    resolved_at timestamptz,
    resolution_note text,
    resolution_version_a_id uuid,
    resolution_version_b_id uuid,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_conflicts__org_id_id UNIQUE (org_id, id),
    CONSTRAINT ck_conflicts__canonical_pair CHECK (claim_a_id < claim_b_id),
    CONSTRAINT ck_conflicts__resolved_fields CHECK (
        (status = 'open' AND resolved_at IS NULL)
        OR (status <> 'open' AND resolved_at IS NOT NULL)
    ),
    CONSTRAINT uq_conflicts__pair UNIQUE (org_id, claim_a_id, claim_b_id),
    CONSTRAINT fk_conflicts__claim_a_org
        FOREIGN KEY (org_id, claim_a_id) REFERENCES claims (org_id, id) ON DELETE RESTRICT,
    CONSTRAINT fk_conflicts__claim_b_org
        FOREIGN KEY (org_id, claim_b_id) REFERENCES claims (org_id, id) ON DELETE RESTRICT,
    CONSTRAINT fk_conflicts__detected_version_org
        FOREIGN KEY (org_id, first_detected_version_id)
        REFERENCES document_versions (org_id, id)
        ON DELETE SET NULL (first_detected_version_id),
    CONSTRAINT fk_conflicts__resolution_a_org
        FOREIGN KEY (org_id, resolution_version_a_id)
        REFERENCES document_versions (org_id, id)
        ON DELETE SET NULL (resolution_version_a_id),
    CONSTRAINT fk_conflicts__resolution_b_org
        FOREIGN KEY (org_id, resolution_version_b_id)
        REFERENCES document_versions (org_id, id)
        ON DELETE SET NULL (resolution_version_b_id)
);

CREATE INDEX idx_conflicts__org_status ON conflicts (org_id, status);
CREATE INDEX idx_conflicts__org_detected ON conflicts (org_id, first_detected_at);

CREATE OR REPLACE FUNCTION conflicts_enforce_lifecycle()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'conflicts history is immutable: DELETE is forbidden'
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;

    -- Immutable identity / detection columns.
    IF NEW.id IS DISTINCT FROM OLD.id
        OR NEW.org_id IS DISTINCT FROM OLD.org_id
        OR NEW.conflict_type IS DISTINCT FROM OLD.conflict_type
        OR NEW.claim_a_id IS DISTINCT FROM OLD.claim_a_id
        OR NEW.claim_b_id IS DISTINCT FROM OLD.claim_b_id
        OR NEW.first_detected_at IS DISTINCT FROM OLD.first_detected_at
        OR NEW.first_detected_version_id IS DISTINCT FROM OLD.first_detected_version_id
        OR NEW.created_at IS DISTINCT FROM OLD.created_at
        OR NEW.severity IS DISTINCT FROM OLD.severity
    THEN
        RAISE EXCEPTION 'conflicts: immutable columns cannot change'
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;

    -- Legal one-way transitions: open → terminal only; terminals are terminal.
    IF OLD.status = 'open' THEN
        IF NEW.status NOT IN ('open', 'resolved', 'accepted_exception', 'false_positive') THEN
            RAISE EXCEPTION 'conflicts: illegal status %', NEW.status
                USING ERRCODE = 'integrity_constraint_violation';
        END IF;
    ELSIF OLD.status IS DISTINCT FROM NEW.status THEN
        RAISE EXCEPTION 'conflicts: terminal status % cannot transition to %', OLD.status, NEW.status
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;

    IF OLD.status <> 'open' AND NEW.status = OLD.status THEN
        -- Allow resolution_note / updated_at touch on terminals? ADR: history immutable.
        IF NEW.resolution_note IS DISTINCT FROM OLD.resolution_note
            OR NEW.resolved_at IS DISTINCT FROM OLD.resolved_at
            OR NEW.resolution_version_a_id IS DISTINCT FROM OLD.resolution_version_a_id
            OR NEW.resolution_version_b_id IS DISTINCT FROM OLD.resolution_version_b_id
        THEN
            RAISE EXCEPTION 'conflicts: terminal resolution fields are immutable'
                USING ERRCODE = 'integrity_constraint_violation';
        END IF;
    END IF;

    NEW.updated_at := clock_timestamp();
    RETURN NEW;
END;
$$;

CREATE TRIGGER trg_conflicts__lifecycle
    BEFORE UPDATE OR DELETE ON conflicts
    FOR EACH ROW
    EXECUTE FUNCTION conflicts_enforce_lifecycle();

CREATE TABLE conflict_evidence (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id uuid NOT NULL REFERENCES orgs(id) ON DELETE RESTRICT,
    conflict_id uuid NOT NULL,
    claim_id uuid NOT NULL,
    evidence_role text NOT NULL CHECK (evidence_role IN (
        'left',
        'right',
        'resolution_left',
        'resolution_right',
        'supporting'
    )),
    citation_quote text,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT uq_conflict_evidence__role UNIQUE (conflict_id, claim_id, evidence_role),
    CONSTRAINT fk_conflict_evidence__conflict_org
        FOREIGN KEY (org_id, conflict_id) REFERENCES conflicts (org_id, id) ON DELETE RESTRICT,
    CONSTRAINT fk_conflict_evidence__claim_org
        FOREIGN KEY (org_id, claim_id) REFERENCES claims (org_id, id) ON DELETE RESTRICT
);

CREATE INDEX idx_conflict_evidence__org_conflict ON conflict_evidence (org_id, conflict_id);

CREATE OR REPLACE FUNCTION conflict_evidence_enforce_immutability()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION 'conflict_evidence is immutable: % is forbidden', TG_OP
        USING ERRCODE = 'integrity_constraint_violation';
END;
$$;

CREATE TRIGGER trg_conflict_evidence__immutability
    BEFORE UPDATE OR DELETE ON conflict_evidence
    FOR EACH ROW
    EXECUTE FUNCTION conflict_evidence_enforce_immutability();
