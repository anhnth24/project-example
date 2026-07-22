//! Tenant-scoped conflict lifecycle queries for REST (schema triggers enforce transitions).

use chrono::{DateTime, Utc};
use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;
use crate::db::models::{
    Conflict, ConflictEvidence, ConflictSeverity, ConflictStatus, ConflictType, EvidenceRole,
    PublicationState,
};

const CONFLICT_COLUMNS: &str =
    "id, org_id, status, severity, conflict_type, claim_a_id, claim_b_id, \
    first_detected_at, first_detected_version_id, resolved_at, resolution_note, \
    resolution_version_a_id, resolution_version_b_id, created_at, updated_at";

/// Lists conflicts whose both claim sides remain in allowed collections.
pub async fn list_authorized_page(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    allowed_collection_ids: &[Uuid],
    status: Option<ConflictStatus>,
    limit: i64,
    after_detected_at: Option<DateTime<Utc>>,
    after_id: Option<Uuid>,
) -> Result<Vec<Conflict>, DbError> {
    if allowed_collection_ids.is_empty() || limit <= 0 {
        return Ok(Vec::new());
    }
    let status = status.map(|value| value.as_str());
    let rows = txn
        .query(
            &format!(
                "SELECT {CONFLICT_COLUMNS}
                 FROM conflicts conf
                 WHERE conf.org_id = $1
                   AND ($2::text IS NULL OR conf.status = $2)
                   AND EXISTS (
                     SELECT 1
                     FROM claims ca
                     JOIN documents da
                       ON da.org_id = ca.org_id AND da.id = ca.document_id
                     WHERE ca.org_id = conf.org_id
                       AND ca.id = conf.claim_a_id
                       AND da.collection_id = ANY($3)
                       AND da.deleted_at IS NULL
                   )
                   AND EXISTS (
                     SELECT 1
                     FROM claims cb
                     JOIN documents db
                       ON db.org_id = cb.org_id AND db.id = cb.document_id
                     WHERE cb.org_id = conf.org_id
                       AND cb.id = conf.claim_b_id
                       AND db.collection_id = ANY($3)
                       AND db.deleted_at IS NULL
                   )
                   AND (
                     $4::timestamptz IS NULL
                     OR (conf.first_detected_at, conf.id) > ($4::timestamptz, $5::uuid)
                   )
                 ORDER BY conf.first_detected_at, conf.id
                 LIMIT $6"
            ),
            &[
                &ctx.org_id(),
                &status,
                &allowed_collection_ids,
                &after_detected_at,
                &after_id,
                &limit,
            ],
        )
        .await?;
    rows.iter().map(map_conflict).collect()
}

pub async fn get_authorized(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    allowed_collection_ids: &[Uuid],
    conflict_id: Uuid,
) -> Result<Conflict, DbError> {
    if allowed_collection_ids.is_empty() {
        return Err(DbError::NotFound);
    }
    let row = txn
        .query_opt(
            &format!(
                "SELECT {CONFLICT_COLUMNS}
                 FROM conflicts conf
                 WHERE conf.org_id = $1
                   AND conf.id = $2
                   AND EXISTS (
                     SELECT 1
                     FROM claims ca
                     JOIN documents da
                       ON da.org_id = ca.org_id AND da.id = ca.document_id
                     WHERE ca.org_id = conf.org_id
                       AND ca.id = conf.claim_a_id
                       AND da.collection_id = ANY($3)
                       AND da.deleted_at IS NULL
                   )
                   AND EXISTS (
                     SELECT 1
                     FROM claims cb
                     JOIN documents db
                       ON db.org_id = cb.org_id AND db.id = cb.document_id
                     WHERE cb.org_id = conf.org_id
                       AND cb.id = conf.claim_b_id
                       AND db.collection_id = ANY($3)
                       AND db.deleted_at IS NULL
                   )"
            ),
            &[&ctx.org_id(), &conflict_id, &allowed_collection_ids],
        )
        .await?
        .ok_or(DbError::NotFound)?;
    map_conflict(&row)
}

/// Input for an open→terminal triage transition.
#[derive(Debug, Clone)]
pub struct TriageConflict<'a> {
    pub conflict_id: Uuid,
    pub status: ConflictStatus,
    pub resolution_note: Option<&'a str>,
    pub resolution_version_a_id: Option<Uuid>,
    pub resolution_version_b_id: Option<Uuid>,
}

/// Applies an open→terminal triage transition enforced by schema triggers.
pub async fn triage(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    allowed_collection_ids: &[Uuid],
    input: TriageConflict<'_>,
) -> Result<Conflict, DbError> {
    if input.status == ConflictStatus::Open {
        return Err(DbError::Config(
            "conflict triage requires a terminal status".into(),
        ));
    }
    let conflict = get_authorized(txn, ctx, allowed_collection_ids, input.conflict_id).await?;
    validate_resolution_versions(
        txn,
        ctx,
        allowed_collection_ids,
        &conflict,
        input.resolution_version_a_id,
        input.resolution_version_b_id,
    )
    .await?;
    let status = input.status.as_str();
    let row = txn
        .query_opt(
            &format!(
                "UPDATE conflicts
                 SET status = $3,
                     resolved_at = clock_timestamp(),
                     resolution_note = $4,
                     resolution_version_a_id = $5,
                     resolution_version_b_id = $6,
                     updated_at = clock_timestamp()
                 WHERE org_id = $1 AND id = $2 AND status = 'open'
                 RETURNING {CONFLICT_COLUMNS}"
            ),
            &[
                &ctx.org_id(),
                &input.conflict_id,
                &status,
                &input.resolution_note,
                &input.resolution_version_a_id,
                &input.resolution_version_b_id,
            ],
        )
        .await?
        .ok_or(DbError::StaleState {
            expected: "open".into(),
            observed: "missing_or_terminal".into(),
        })?;
    map_conflict(&row)
}

async fn validate_resolution_versions(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    allowed_collection_ids: &[Uuid],
    conflict: &Conflict,
    resolution_version_a_id: Option<Uuid>,
    resolution_version_b_id: Option<Uuid>,
) -> Result<(), DbError> {
    let claim_a_document = claim_document_id(txn, ctx, conflict.claim_a_id).await?;
    let claim_b_document = claim_document_id(txn, ctx, conflict.claim_b_id).await?;
    if let Some(version_id) = resolution_version_a_id {
        ensure_published_version_for_document(
            txn,
            ctx,
            allowed_collection_ids,
            claim_a_document,
            version_id,
        )
        .await?;
    }
    if let Some(version_id) = resolution_version_b_id {
        ensure_published_version_for_document(
            txn,
            ctx,
            allowed_collection_ids,
            claim_b_document,
            version_id,
        )
        .await?;
    }
    Ok(())
}

async fn claim_document_id(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    claim_id: Uuid,
) -> Result<Uuid, DbError> {
    let row = txn
        .query_opt(
            "SELECT document_id
             FROM claims
             WHERE org_id = $1 AND id = $2",
            &[&ctx.org_id(), &claim_id],
        )
        .await?
        .ok_or(DbError::Config("invalid_resolution_version".into()))?;
    Ok(row.get(0))
}

async fn ensure_published_version_for_document(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    allowed_collection_ids: &[Uuid],
    document_id: Uuid,
    version_id: Uuid,
) -> Result<(), DbError> {
    let row = txn
        .query_opt(
            "SELECT d.collection_id, dv.publication_state, d.deleted_at
             FROM document_versions dv
             JOIN documents d
               ON d.org_id = dv.org_id AND d.id = dv.document_id
             WHERE dv.org_id = $1
               AND dv.document_id = $2
               AND dv.id = $3",
            &[&ctx.org_id(), &document_id, &version_id],
        )
        .await?
        .ok_or(DbError::Config("invalid_resolution_version".into()))?;
    let collection_id: Uuid = row.get("collection_id");
    let publication_state: String = row.get("publication_state");
    let deleted_at: Option<DateTime<Utc>> = row.get("deleted_at");
    if deleted_at.is_some() || !allowed_collection_ids.contains(&collection_id) {
        return Err(DbError::Config("invalid_resolution_version".into()));
    }
    let state = PublicationState::parse(&publication_state)
        .map_err(|_| DbError::Config("invalid_resolution_version".into()))?;
    if state != PublicationState::Published {
        return Err(DbError::Config("invalid_resolution_version".into()));
    }
    Ok(())
}

/// Authorized evidence page: every claim's document/collection is checked before
/// any citation quote is returned. Unauthorized claim rows are omitted.
///
/// Fresh ACL mirrors authorized conflict evidence in `search`: the claim's pinned
/// `document_versions` row selects `qa.query` vs `qa.history` via
/// `CASE WHEN dv.is_current THEN 'qa.query' ELSE 'qa.history' END`.
pub async fn list_authorized_evidence_page(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    allowed_collection_ids: &[Uuid],
    conflict_id: Uuid,
    limit: i64,
    after_created_at: Option<DateTime<Utc>>,
    after_id: Option<Uuid>,
) -> Result<Vec<ConflictEvidence>, DbError> {
    if allowed_collection_ids.is_empty() || limit <= 0 {
        return Ok(Vec::new());
    }
    // Fail closed if the conflict itself is out of scope.
    let _ = get_authorized(txn, ctx, allowed_collection_ids, conflict_id).await?;
    let rows = txn
        .query(
            "SELECT ce.id, ce.org_id, ce.conflict_id, ce.claim_id, ce.evidence_role,
                    ce.citation_quote, ce.created_at
             FROM conflict_evidence ce
             JOIN claims c
               ON c.org_id = ce.org_id AND c.id = ce.claim_id
             JOIN documents d
               ON d.org_id = c.org_id AND d.id = c.document_id
             JOIN document_versions dv
               ON dv.org_id = c.org_id
              AND dv.document_id = c.document_id
              AND dv.id = c.version_id
             WHERE ce.org_id = $1
               AND ce.conflict_id = $2
               AND d.collection_id = ANY($3)
               AND d.deleted_at IS NULL
               AND d.state = 'indexed'
               AND dv.publication_state = 'published'
               AND EXISTS (
                 SELECT 1
                 FROM collections acl_c
                 JOIN org_memberships acl_m
                   ON acl_m.org_id = acl_c.org_id AND acl_m.user_id = $4
                 JOIN users acl_u ON acl_u.id = acl_m.user_id
                 JOIN roles acl_r
                   ON acl_r.org_id = acl_m.org_id AND acl_r.code = acl_m.role
                 JOIN role_permissions acl_rp
                   ON acl_rp.org_id = acl_r.org_id AND acl_rp.role_id = acl_r.id
                 JOIN permissions acl_p ON acl_p.id = acl_rp.permission_id
                 WHERE acl_c.org_id = d.org_id
                   AND acl_c.id = d.collection_id
                   AND acl_c.deleted_at IS NULL
                   AND acl_u.disabled_at IS NULL
                   AND acl_p.code = CASE WHEN dv.is_current THEN 'qa.query' ELSE 'qa.history' END
                   AND EXISTS (
                     SELECT 1
                     FROM role_permissions query_rp
                     JOIN permissions query_p ON query_p.id = query_rp.permission_id
                     WHERE query_rp.org_id = acl_r.org_id
                       AND query_rp.role_id = acl_r.id
                       AND query_p.code = 'qa.query'
                   )
                   AND (
                     acl_c.visibility = 'org'
                     OR acl_c.owner_user_id = $4
                     OR EXISTS (
                       SELECT 1 FROM collection_user_access cua
                       WHERE cua.org_id = acl_c.org_id
                         AND cua.collection_id = acl_c.id
                         AND cua.user_id = $4
                     )
                   )
               )
               AND (
                 $5::timestamptz IS NULL
                 OR (ce.created_at, ce.id) > ($5::timestamptz, $6::uuid)
               )
             ORDER BY ce.created_at, ce.id
             LIMIT $7",
            &[
                &ctx.org_id(),
                &conflict_id,
                &allowed_collection_ids,
                &ctx.user_id(),
                &after_created_at,
                &after_id,
                &limit,
            ],
        )
        .await?;
    rows.iter().map(map_evidence).collect()
}

fn map_conflict(row: &Row) -> Result<Conflict, DbError> {
    let status: String = row.get("status");
    let severity: String = row.get("severity");
    let conflict_type: String = row.get("conflict_type");
    Ok(Conflict {
        id: row.get("id"),
        org_id: row.get("org_id"),
        status: ConflictStatus::parse(&status).map_err(DbError::Config)?,
        severity: ConflictSeverity::parse(&severity).map_err(DbError::Config)?,
        conflict_type: ConflictType::parse(&conflict_type).map_err(DbError::Config)?,
        claim_a_id: row.get("claim_a_id"),
        claim_b_id: row.get("claim_b_id"),
        first_detected_at: row.get("first_detected_at"),
        first_detected_version_id: row.get("first_detected_version_id"),
        resolved_at: row.get("resolved_at"),
        resolution_note: row.get("resolution_note"),
        resolution_version_a_id: row.get("resolution_version_a_id"),
        resolution_version_b_id: row.get("resolution_version_b_id"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn map_evidence(row: &Row) -> Result<ConflictEvidence, DbError> {
    let evidence_role: String = row.get("evidence_role");
    Ok(ConflictEvidence {
        id: row.get("id"),
        org_id: row.get("org_id"),
        conflict_id: row.get("conflict_id"),
        claim_id: row.get("claim_id"),
        evidence_role: EvidenceRole::parse(&evidence_role).map_err(DbError::Config)?,
        citation_quote: row.get("citation_quote"),
        created_at: row.get("created_at"),
    })
}
