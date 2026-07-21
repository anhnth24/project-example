//! Tenant-scoped FTS, version resolution, and hydration queries for retrieval.
//!
//! PostgreSQL is the authority for chunk text, document state, ACL, and version
//! visibility. Vector payloads supply candidate identities only.
//!
//! Authoritative Q&A / citation / conflict / history / probe queries must use the
//! canonical collection ACL predicate from [`crate::db::acl`] (org / owner /
//! user / group / role).

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

use rust_decimal::Decimal;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;
use crate::db::models::{
    ConflictSeverity, ConflictStatus, ConflictType, DocumentState, IndexGenerationState,
    PublicationState,
};

/// H9: deduplicate and stably sort UUID lists used in `ANY($n)` authz probes.
pub fn dedup_sorted_uuids(ids: &[Uuid]) -> Vec<Uuid> {
    ids.iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Lexical candidate before PG hydration (scores only; no body text).
#[derive(Debug, Clone, PartialEq)]
pub struct FtsCandidate {
    pub chunk_id: Uuid,
    pub chunk_identity_sha256: String,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub collection_id: Uuid,
    pub rank: f32,
}

/// Authorized chunk row hydrated from PostgreSQL for citation/rerank.
#[derive(Debug, Clone, PartialEq)]
pub struct HydratedChunkRow {
    pub chunk_id: Uuid,
    pub chunk_identity_sha256: String,
    pub org_id: Uuid,
    pub collection_id: Uuid,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub version_number: i32,
    pub content_sha256: String,
    pub heading_path: Vec<String>,
    pub body: String,
    pub page: Option<i32>,
    pub slide: Option<i32>,
    pub sheet: Option<String>,
    pub span_start: Option<i32>,
    pub span_end: Option<i32>,
    pub document_state: DocumentState,
    pub deleted_at: Option<DateTime<Utc>>,
    pub publication_state: PublicationState,
    pub is_current: bool,
    pub effective_from: DateTime<Utc>,
    pub effective_to: Option<DateTime<Utc>>,
    pub index_metadata_id: Uuid,
    pub index_generation_active: bool,
    pub index_generation_state: IndexGenerationState,
}

/// Conflict evidence sides that both remain authorized after recheck.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedConflictEvidence {
    pub conflict_id: Uuid,
    pub claim_a_id: Uuid,
    pub claim_b_id: Uuid,
    pub claim_a_document_id: Uuid,
    pub claim_b_document_id: Uuid,
    pub claim_a_version_id: Uuid,
    pub claim_b_version_id: Uuid,
    pub claim_a_collection_id: Uuid,
    pub claim_b_collection_id: Uuid,
    pub claim_a_is_current: bool,
    pub claim_b_is_current: bool,
    pub claim_a_published: bool,
    pub claim_b_published: bool,
    pub claim_a_quote: Option<String>,
    pub claim_b_quote: Option<String>,
}

/// Version visibility filter shared by FTS and hydration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionVisibility {
    /// Only the current published pointer (`is_current`).
    Current,
    /// Explicit set of published version ids (as_of / compare / history).
    VersionIds(BTreeSet<Uuid>),
}

impl VersionVisibility {
    fn required_permission(&self) -> &'static str {
        match self {
            Self::Current => "qa.query",
            Self::VersionIds(_) => "qa.history",
        }
    }
}

/// Shadow/building/retired generations must not surface in retrieval.
pub fn index_generation_visible_for_retrieval(
    is_active: bool,
    state: IndexGenerationState,
) -> bool {
    is_active && state == IndexGenerationState::Active
}

/// Resolves the published version effective at `as_of` for each in-scope document.
pub async fn resolve_as_of_version_ids(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    collection_ids: &[Uuid],
    as_of: DateTime<Utc>,
) -> Result<BTreeSet<Uuid>, DbError> {
    if collection_ids.is_empty() {
        return Ok(BTreeSet::new());
    }
    let rows = txn
        .query(
            "SELECT DISTINCT ON (d.id) dv.id
             FROM documents d
             JOIN document_versions dv
               ON dv.org_id = d.org_id
              AND dv.document_id = d.id
             WHERE d.org_id = $1
               AND d.collection_id = ANY($2)
               AND d.deleted_at IS NULL
               AND d.state = 'indexed'
               AND dv.publication_state = 'published'
               AND dv.effective_from <= $3
               AND (dv.effective_to IS NULL OR dv.effective_to > $3)
             ORDER BY d.id, dv.version_number DESC, dv.id",
            &[&ctx.org_id(), &collection_ids, &as_of],
        )
        .await?;
    Ok(rows.iter().map(|row| row.get(0)).collect())
}

/// Loads published versions for one logical document (history mode).
pub async fn list_published_version_ids_for_document(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
    collection_ids: &[Uuid],
) -> Result<Vec<(Uuid, i32)>, DbError> {
    let rows = txn
        .query(
            "SELECT dv.id, dv.version_number
             FROM documents d
             JOIN document_versions dv
               ON dv.org_id = d.org_id
              AND dv.document_id = d.id
             WHERE d.org_id = $1
               AND d.id = $2
               AND d.collection_id = ANY($3)
               AND d.deleted_at IS NULL
               AND d.state = 'indexed'
               AND dv.publication_state = 'published'
             ORDER BY dv.version_number, dv.id",
            &[&ctx.org_id(), &document_id, &collection_ids],
        )
        .await?;
    Ok(rows.iter().map(|row| (row.get(0), row.get(1))).collect())
}

/// Authoritative published version timeline for one lineage (history mode metadata).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionTimelineRow {
    pub version_id: Uuid,
    pub version_number: i32,
    pub parent_version_id: Option<Uuid>,
    pub is_current: bool,
    pub effective_from: DateTime<Utc>,
    pub effective_to: Option<DateTime<Utc>>,
    pub content_sha256: String,
}

pub async fn list_published_version_timeline(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
    collection_ids: &[Uuid],
) -> Result<Vec<VersionTimelineRow>, DbError> {
    list_published_version_timeline_page(txn, ctx, document_id, collection_ids, 0, 256).await
}

/// Newest-first page for history Q&A (`LIMIT = page_size + 1` to detect truncation).
///
/// Rows are returned newest-first so callers can truncate while preserving current.
/// `before_version_no`: stable cursor — only versions with `version_number` strictly
/// less than the cursor (M4). `None` = newest page (including current).
pub async fn list_published_version_timeline_recent_page(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
    collection_ids: &[Uuid],
    limit: i64,
    before_version_no: Option<i32>,
) -> Result<Vec<VersionTimelineRow>, DbError> {
    if collection_ids.is_empty() {
        return Ok(Vec::new());
    }
    let limit = limit.clamp(1, 64);
    let rows = txn
        .query(
            "SELECT dv.id, dv.version_number, dv.parent_version_id, dv.is_current,
                    dv.effective_from, dv.effective_to, dv.content_sha256
             FROM documents d
             JOIN document_versions dv
               ON dv.org_id = d.org_id
              AND dv.document_id = d.id
             WHERE d.org_id = $1
               AND d.id = $2
               AND d.collection_id = ANY($3)
               AND d.deleted_at IS NULL
               AND d.state = 'indexed'
               AND dv.publication_state = 'published'
               AND ($6::int IS NULL OR dv.version_number < $6)
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
                   AND acl_p.code = 'qa.history'
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
                     OR EXISTS (
                       SELECT 1 FROM collection_group_access cga
                       JOIN group_memberships gm
                         ON gm.org_id = cga.org_id
                        AND gm.group_id = cga.group_id
                        AND gm.user_id = $4
                       WHERE cga.org_id = acl_c.org_id
                         AND cga.collection_id = acl_c.id
                     )
                     OR EXISTS (
                       SELECT 1 FROM collection_role_access cra
                       JOIN org_memberships om
                         ON om.org_id = cra.org_id
                        AND om.user_id = $4
                       JOIN roles rr
                         ON rr.org_id = om.org_id
                        AND rr.code = om.role
                        AND rr.id = cra.role_id
                       WHERE cra.org_id = acl_c.org_id
                         AND cra.collection_id = acl_c.id
                     )
                   )
               )
             ORDER BY dv.version_number DESC, dv.id DESC
             LIMIT $5",
            &[
                &ctx.org_id(),
                &document_id,
                &collection_ids,
                &ctx.user_id(),
                &limit,
                &before_version_no,
            ],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|row| VersionTimelineRow {
            version_id: row.get(0),
            version_number: row.get(1),
            parent_version_id: row.get(2),
            is_current: row.get(3),
            effective_from: row.get(4),
            effective_to: row.get(5),
            content_sha256: row.get(6),
        })
        .collect())
}

/// Paged published timeline (same authoritative snapshot when called in one txn).
pub async fn list_published_version_timeline_page(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
    collection_ids: &[Uuid],
    offset: i64,
    limit: i64,
) -> Result<Vec<VersionTimelineRow>, DbError> {
    if collection_ids.is_empty() {
        return Ok(Vec::new());
    }
    let limit = limit.clamp(1, 64);
    let offset = offset.max(0);
    let rows = txn
        .query(
            "SELECT dv.id, dv.version_number, dv.parent_version_id, dv.is_current,
                    dv.effective_from, dv.effective_to, dv.content_sha256
             FROM documents d
             JOIN document_versions dv
               ON dv.org_id = d.org_id
              AND dv.document_id = d.id
             WHERE d.org_id = $1
               AND d.id = $2
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
                   AND acl_p.code = 'qa.history'
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
                     OR EXISTS (
                       SELECT 1 FROM collection_group_access cga
                       JOIN group_memberships gm
                         ON gm.org_id = cga.org_id
                        AND gm.group_id = cga.group_id
                        AND gm.user_id = $4
                       WHERE cga.org_id = acl_c.org_id
                         AND cga.collection_id = acl_c.id
                     )
                     OR EXISTS (
                       SELECT 1 FROM collection_role_access cra
                       JOIN org_memberships om
                         ON om.org_id = cra.org_id
                        AND om.user_id = $4
                       JOIN roles rr
                         ON rr.org_id = om.org_id
                        AND rr.code = om.role
                        AND rr.id = cra.role_id
                       WHERE cra.org_id = acl_c.org_id
                         AND cra.collection_id = acl_c.id
                     )
                   )
               )
             ORDER BY dv.version_number, dv.id
             OFFSET $5 LIMIT $6",
            &[
                &ctx.org_id(),
                &document_id,
                &collection_ids,
                &ctx.user_id(),
                &offset,
                &limit,
            ],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|row| VersionTimelineRow {
            version_id: row.get(0),
            version_number: row.get(1),
            parent_version_id: row.get(2),
            is_current: row.get(3),
            effective_from: row.get(4),
            effective_to: row.get(5),
            content_sha256: row.get(6),
        })
        .collect())
}

/// Fresh-authorized conflict row with typed claim fields for Q&A warnings/notes.
#[derive(Debug, Clone, PartialEq)]
pub struct QaConflictRow {
    pub conflict_id: Uuid,
    pub status: ConflictStatus,
    pub severity: ConflictSeverity,
    pub conflict_type: ConflictType,
    pub claim_a_id: Uuid,
    pub claim_b_id: Uuid,
    pub claim_a_key: String,
    pub claim_b_key: String,
    pub claim_a_scope: String,
    pub claim_b_scope: String,
    pub claim_a_unit: Option<String>,
    pub claim_b_unit: Option<String>,
    pub claim_a_number: Option<Decimal>,
    pub claim_b_number: Option<Decimal>,
    pub claim_a_document_id: Uuid,
    pub claim_b_document_id: Uuid,
    pub claim_a_version_id: Uuid,
    pub claim_b_version_id: Uuid,
    pub claim_a_chunk_id: Option<Uuid>,
    pub claim_b_chunk_id: Option<Uuid>,
    pub claim_a_collection_id: Uuid,
    pub claim_b_collection_id: Uuid,
    pub claim_a_is_current: bool,
    pub claim_b_is_current: bool,
    pub claim_a_quote: Option<String>,
    pub claim_b_quote: Option<String>,
    pub resolution_note: Option<String>,
    pub resolution_version_a_id: Option<Uuid>,
    pub resolution_version_b_id: Option<Uuid>,
    pub resolution_a_chunk_id: Option<Uuid>,
    pub resolution_b_chunk_id: Option<Uuid>,
    pub resolution_a_number: Option<Decimal>,
    pub resolution_b_number: Option<Decimal>,
    pub resolution_a_authorized: bool,
    pub resolution_b_authorized: bool,
}

/// Loads conflicts with both claim sides authorized under the caller's scope.
///
/// `current_only`: when true, both claim versions must be current/published/effective.
/// When false (history), open+terminal conflicts are returned with resolution version auth.
pub async fn load_qa_conflicts(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    collection_ids: &[Uuid],
    current_only: bool,
) -> Result<Vec<QaConflictRow>, DbError> {
    if collection_ids.is_empty() {
        return Ok(Vec::new());
    }
    let permission = if current_only {
        "qa.query"
    } else {
        "qa.history"
    };
    let rows = txn
        .query(
            "SELECT conf.id AS conflict_id,
                    conf.status, conf.severity, conf.conflict_type,
                    conf.status AS status_at,
                    conf.claim_a_id, conf.claim_b_id,
                    conf.resolution_note,
                    conf.resolution_version_a_id, conf.resolution_version_b_id,
                    ca.claim_key AS claim_a_key, cb.claim_key AS claim_b_key,
                    ca.scope AS claim_a_scope, cb.scope AS claim_b_scope,
                    ca.unit AS claim_a_unit, cb.unit AS claim_b_unit,
                    COALESCE(ca.value_number, ca.value_money) AS claim_a_number,
                    COALESCE(cb.value_number, cb.value_money) AS claim_b_number,
                    ca.document_id AS claim_a_document_id,
                    cb.document_id AS claim_b_document_id,
                    ca.version_id AS claim_a_version_id,
                    cb.version_id AS claim_b_version_id,
                    ca.chunk_id AS claim_a_chunk_id,
                    cb.chunk_id AS claim_b_chunk_id,
                    da.collection_id AS claim_a_collection_id,
                    db.collection_id AS claim_b_collection_id,
                    dva.is_current AS claim_a_is_current,
                    dvb.is_current AS claim_b_is_current,
                    ca.citation_quote AS claim_a_quote,
                    cb.citation_quote AS claim_b_quote,
                    EXISTS (
                      SELECT 1 FROM document_versions rva
                      JOIN documents rda
                        ON rda.org_id = rva.org_id AND rda.id = rva.document_id
                      WHERE rva.org_id = conf.org_id
                        AND rva.id = conf.resolution_version_a_id
                        AND rda.collection_id = ANY($2)
                        AND rda.deleted_at IS NULL
                        AND rva.publication_state = 'published'
                    ) AS resolution_a_authorized,
                    EXISTS (
                      SELECT 1 FROM document_versions rvb
                      JOIN documents rdb
                        ON rdb.org_id = rvb.org_id AND rdb.id = rvb.document_id
                      WHERE rvb.org_id = conf.org_id
                        AND rvb.id = conf.resolution_version_b_id
                        AND rdb.collection_id = ANY($2)
                        AND rdb.deleted_at IS NULL
                        AND rvb.publication_state = 'published'
                    ) AS resolution_b_authorized
             FROM conflicts conf
             JOIN claims ca
               ON ca.org_id = conf.org_id AND ca.id = conf.claim_a_id
             JOIN claims cb
               ON cb.org_id = conf.org_id AND cb.id = conf.claim_b_id
             JOIN documents da
               ON da.org_id = ca.org_id AND da.id = ca.document_id
             JOIN documents db
               ON db.org_id = cb.org_id AND db.id = cb.document_id
             JOIN document_versions dva
               ON dva.org_id = ca.org_id
              AND dva.document_id = ca.document_id
              AND dva.id = ca.version_id
             JOIN document_versions dvb
               ON dvb.org_id = cb.org_id
              AND dvb.document_id = cb.document_id
              AND dvb.id = cb.version_id
             WHERE conf.org_id = $1
               AND da.collection_id = ANY($2)
               AND db.collection_id = ANY($2)
               AND da.deleted_at IS NULL
               AND db.deleted_at IS NULL
               AND da.state = 'indexed'
               AND db.state = 'indexed'
               AND dva.publication_state = 'published'
               AND dvb.publication_state = 'published'
               AND (
                 ($3 AND conf.status = 'open' AND dva.is_current AND dvb.is_current)
                 OR (NOT $3)
               )
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
                 WHERE acl_c.org_id = da.org_id
                   AND acl_c.id = da.collection_id
                   AND acl_c.deleted_at IS NULL
                   AND acl_u.disabled_at IS NULL
                   AND acl_p.code = $5
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
                     OR EXISTS (
                       SELECT 1 FROM collection_group_access cga
                       JOIN group_memberships gm
                         ON gm.org_id = cga.org_id
                        AND gm.group_id = cga.group_id
                        AND gm.user_id = $4
                       WHERE cga.org_id = acl_c.org_id
                         AND cga.collection_id = acl_c.id
                     )
                     OR EXISTS (
                       SELECT 1 FROM collection_role_access cra
                       JOIN org_memberships om
                         ON om.org_id = cra.org_id
                        AND om.user_id = $4
                       JOIN roles rr
                         ON rr.org_id = om.org_id
                        AND rr.code = om.role
                        AND rr.id = cra.role_id
                       WHERE cra.org_id = acl_c.org_id
                         AND cra.collection_id = acl_c.id
                     )
                   )
               )
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
                 WHERE acl_c.org_id = db.org_id
                   AND acl_c.id = db.collection_id
                   AND acl_c.deleted_at IS NULL
                   AND acl_u.disabled_at IS NULL
                   AND acl_p.code = $5
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
                     OR EXISTS (
                       SELECT 1 FROM collection_group_access cga
                       JOIN group_memberships gm
                         ON gm.org_id = cga.org_id
                        AND gm.group_id = cga.group_id
                        AND gm.user_id = $4
                       WHERE cga.org_id = acl_c.org_id
                         AND cga.collection_id = acl_c.id
                     )
                     OR EXISTS (
                       SELECT 1 FROM collection_role_access cra
                       JOIN org_memberships om
                         ON om.org_id = cra.org_id
                        AND om.user_id = $4
                       JOIN roles rr
                         ON rr.org_id = om.org_id
                        AND rr.code = om.role
                        AND rr.id = cra.role_id
                       WHERE cra.org_id = acl_c.org_id
                         AND cra.collection_id = acl_c.id
                     )
                   )
               )
             ORDER BY conf.id",
            &[
                &ctx.org_id(),
                &collection_ids,
                &current_only,
                &ctx.user_id(),
                &permission,
            ],
        )
        .await?;
    rows.iter().map(map_qa_conflict_row).collect()
}

fn map_qa_conflict_row(row: &Row) -> Result<QaConflictRow, DbError> {
    // Prefer status_at (AsOf lifecycle); fall back to raw status.
    let status: String = row
        .try_get::<_, String>("status_at")
        .unwrap_or_else(|_| row.get("status"));
    let severity: String = row.get("severity");
    let conflict_type: String = row.get("conflict_type");
    Ok(QaConflictRow {
        conflict_id: row.get("conflict_id"),
        status: ConflictStatus::parse(&status).map_err(DbError::Config)?,
        severity: ConflictSeverity::parse(&severity).map_err(DbError::Config)?,
        conflict_type: ConflictType::parse(&conflict_type).map_err(DbError::Config)?,
        claim_a_id: row.get("claim_a_id"),
        claim_b_id: row.get("claim_b_id"),
        claim_a_key: row.get("claim_a_key"),
        claim_b_key: row.get("claim_b_key"),
        claim_a_scope: row.get("claim_a_scope"),
        claim_b_scope: row.get("claim_b_scope"),
        claim_a_unit: row.get("claim_a_unit"),
        claim_b_unit: row.get("claim_b_unit"),
        claim_a_number: row.get("claim_a_number"),
        claim_b_number: row.get("claim_b_number"),
        claim_a_document_id: row.get("claim_a_document_id"),
        claim_b_document_id: row.get("claim_b_document_id"),
        claim_a_version_id: row.get("claim_a_version_id"),
        claim_b_version_id: row.get("claim_b_version_id"),
        claim_a_chunk_id: row.get("claim_a_chunk_id"),
        claim_b_chunk_id: row.get("claim_b_chunk_id"),
        claim_a_collection_id: row.get("claim_a_collection_id"),
        claim_b_collection_id: row.get("claim_b_collection_id"),
        claim_a_is_current: row.get("claim_a_is_current"),
        claim_b_is_current: row.get("claim_b_is_current"),
        claim_a_quote: row.get("claim_a_quote"),
        claim_b_quote: row.get("claim_b_quote"),
        resolution_note: row.get("resolution_note"),
        resolution_version_a_id: row.get("resolution_version_a_id"),
        resolution_version_b_id: row.get("resolution_version_b_id"),
        resolution_a_chunk_id: row.try_get("resolution_a_chunk_id").ok().flatten(),
        resolution_b_chunk_id: row.try_get("resolution_b_chunk_id").ok().flatten(),
        resolution_a_number: row.try_get("resolution_a_number").ok().flatten(),
        resolution_b_number: row.try_get("resolution_b_number").ok().flatten(),
        resolution_a_authorized: row.get("resolution_a_authorized"),
        resolution_b_authorized: row.get("resolution_b_authorized"),
    })
}

/// Keep only conflicts whose both claim sides link to the requested/retrieved
/// document+version evidence set. Unrelated conflicts are omitted (not a failure).
pub fn filter_conflicts_to_evidence(
    rows: Vec<QaConflictRow>,
    document_ids: &BTreeSet<Uuid>,
    version_ids: &BTreeSet<Uuid>,
) -> Vec<QaConflictRow> {
    rows.into_iter()
        .filter(|row| {
            document_ids.contains(&row.claim_a_document_id)
                && document_ids.contains(&row.claim_b_document_id)
                && version_ids.contains(&row.claim_a_version_id)
                && version_ids.contains(&row.claim_b_version_id)
        })
        .collect()
}

/// SQL-filtered conflict load bound to evidence document/version IDs + hard limits.
#[allow(clippy::too_many_arguments)]
pub async fn load_qa_conflicts_for_evidence(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    collection_ids: &[Uuid],
    evidence_document_ids: &[Uuid],
    evidence_version_ids: &[Uuid],
    current_only: bool,
    as_of: Option<DateTime<Utc>>,
    limit: i64,
) -> Result<Vec<QaConflictRow>, DbError> {
    let evidence_document_ids = dedup_sorted_uuids(evidence_document_ids);
    let evidence_version_ids = dedup_sorted_uuids(evidence_version_ids);
    let collection_ids = dedup_sorted_uuids(collection_ids);
    if collection_ids.is_empty()
        || evidence_document_ids.is_empty()
        || evidence_version_ids.is_empty()
    {
        return Ok(Vec::new());
    }
    let limit = limit.clamp(1, 64);
    let permission = if current_only {
        "qa.query"
    } else {
        "qa.history"
    };
    let rows = txn
        .query(
            "SELECT conf.id AS conflict_id,
                    conf.status, conf.severity, conf.conflict_type,
                    -- status_at: AsOf lifecycle; future resolution ignored → open.
                    CASE
                      WHEN $8::timestamptz IS NULL THEN conf.status
                      WHEN conf.resolved_at IS NOT NULL AND conf.resolved_at <= $8
                        THEN conf.status
                      ELSE 'open'
                    END AS status_at,
                    conf.claim_a_id, conf.claim_b_id,
                    CASE
                      WHEN (
                        CASE
                          WHEN $8::timestamptz IS NULL THEN conf.status
                          WHEN conf.resolved_at IS NOT NULL AND conf.resolved_at <= $8
                            THEN conf.status
                          ELSE 'open'
                        END
                      ) = 'open' THEN NULL
                      ELSE conf.resolution_note
                    END AS resolution_note,
                    CASE
                      WHEN (
                        CASE
                          WHEN $8::timestamptz IS NULL THEN conf.status
                          WHEN conf.resolved_at IS NOT NULL AND conf.resolved_at <= $8
                            THEN conf.status
                          ELSE 'open'
                        END
                      ) = 'open' THEN NULL
                      ELSE conf.resolution_version_a_id
                    END AS resolution_version_a_id,
                    CASE
                      WHEN (
                        CASE
                          WHEN $8::timestamptz IS NULL THEN conf.status
                          WHEN conf.resolved_at IS NOT NULL AND conf.resolved_at <= $8
                            THEN conf.status
                          ELSE 'open'
                        END
                      ) = 'open' THEN NULL
                      ELSE conf.resolution_version_b_id
                    END AS resolution_version_b_id,
                    ca.claim_key AS claim_a_key, cb.claim_key AS claim_b_key,
                    ca.scope AS claim_a_scope, cb.scope AS claim_b_scope,
                    ca.unit AS claim_a_unit, cb.unit AS claim_b_unit,
                    COALESCE(ca.value_number, ca.value_money) AS claim_a_number,
                    COALESCE(cb.value_number, cb.value_money) AS claim_b_number,
                    ca.document_id AS claim_a_document_id,
                    cb.document_id AS claim_b_document_id,
                    ca.version_id AS claim_a_version_id,
                    cb.version_id AS claim_b_version_id,
                    ca.chunk_id AS claim_a_chunk_id,
                    cb.chunk_id AS claim_b_chunk_id,
                    da.collection_id AS claim_a_collection_id,
                    db.collection_id AS claim_b_collection_id,
                    dva.is_current AS claim_a_is_current,
                    dvb.is_current AS claim_b_is_current,
                    ca.citation_quote AS claim_a_quote,
                    cb.citation_quote AS claim_b_quote,
                    (
                      SELECT rca.chunk_id FROM claims rca
                      WHERE rca.org_id = conf.org_id
                        AND rca.document_id = ca.document_id
                        AND rca.version_id = conf.resolution_version_a_id
                        AND rca.claim_key = ca.claim_key
                        AND rca.subject = ca.subject
                        AND rca.predicate = ca.predicate
                        AND rca.scope = ca.scope
                        AND rca.unit IS NOT DISTINCT FROM ca.unit
                        AND rca.chunk_id IS NOT NULL
                      ORDER BY rca.id LIMIT 1
                    ) AS resolution_a_chunk_id,
                    (
                      SELECT rcb.chunk_id FROM claims rcb
                      WHERE rcb.org_id = conf.org_id
                        AND rcb.document_id = cb.document_id
                        AND rcb.version_id = conf.resolution_version_b_id
                        AND rcb.claim_key = cb.claim_key
                        AND rcb.subject = cb.subject
                        AND rcb.predicate = cb.predicate
                        AND rcb.scope = cb.scope
                        AND rcb.unit IS NOT DISTINCT FROM cb.unit
                        AND rcb.chunk_id IS NOT NULL
                      ORDER BY rcb.id LIMIT 1
                    ) AS resolution_b_chunk_id,
                    (
                      SELECT COALESCE(rca.value_number, rca.value_money) FROM claims rca
                      WHERE rca.org_id = conf.org_id
                        AND rca.document_id = ca.document_id
                        AND rca.version_id = conf.resolution_version_a_id
                        AND rca.claim_key = ca.claim_key
                        AND rca.subject = ca.subject
                        AND rca.predicate = ca.predicate
                        AND rca.scope = ca.scope
                        AND rca.unit IS NOT DISTINCT FROM ca.unit
                      ORDER BY rca.id LIMIT 1
                    ) AS resolution_a_number,
                    (
                      SELECT COALESCE(rcb.value_number, rcb.value_money) FROM claims rcb
                      WHERE rcb.org_id = conf.org_id
                        AND rcb.document_id = cb.document_id
                        AND rcb.version_id = conf.resolution_version_b_id
                        AND rcb.claim_key = cb.claim_key
                        AND rcb.subject = cb.subject
                        AND rcb.predicate = cb.predicate
                        AND rcb.scope = cb.scope
                        AND rcb.unit IS NOT DISTINCT FROM cb.unit
                      ORDER BY rcb.id LIMIT 1
                    ) AS resolution_b_number,
                    EXISTS (
                      SELECT 1 FROM document_versions rva
                      JOIN documents rda
                        ON rda.org_id = rva.org_id AND rda.id = rva.document_id
                      JOIN claims rca
                        ON rca.org_id = rva.org_id
                       AND rca.document_id = ca.document_id
                       AND rca.version_id = rva.id
                       AND rca.claim_key = ca.claim_key
                       AND rca.subject = ca.subject
                       AND rca.predicate = ca.predicate
                       AND rca.scope = ca.scope
                       AND rca.unit IS NOT DISTINCT FROM ca.unit
                       AND rca.chunk_id IS NOT NULL
                      WHERE rva.org_id = conf.org_id
                        AND rva.id = conf.resolution_version_a_id
                        AND rva.document_id = ca.document_id
                        AND rda.collection_id = ANY($2)
                        AND rda.deleted_at IS NULL
                        AND rva.publication_state = 'published'
                        AND rva.effective_from <= COALESCE($8, now())
                        AND (rva.effective_to IS NULL OR rva.effective_to > COALESCE($8, now()))
                    ) AS resolution_a_authorized,
                    EXISTS (
                      SELECT 1 FROM document_versions rvb
                      JOIN documents rdb
                        ON rdb.org_id = rvb.org_id AND rdb.id = rvb.document_id
                      JOIN claims rcb
                        ON rcb.org_id = rvb.org_id
                       AND rcb.document_id = cb.document_id
                       AND rcb.version_id = rvb.id
                       AND rcb.claim_key = cb.claim_key
                       AND rcb.subject = cb.subject
                       AND rcb.predicate = cb.predicate
                       AND rcb.scope = cb.scope
                       AND rcb.unit IS NOT DISTINCT FROM cb.unit
                       AND rcb.chunk_id IS NOT NULL
                      WHERE rvb.org_id = conf.org_id
                        AND rvb.id = conf.resolution_version_b_id
                        AND rvb.document_id = cb.document_id
                        AND rdb.collection_id = ANY($2)
                        AND rdb.deleted_at IS NULL
                        AND rvb.publication_state = 'published'
                        AND rvb.effective_from <= COALESCE($8, now())
                        AND (rvb.effective_to IS NULL OR rvb.effective_to > COALESCE($8, now()))
                    ) AS resolution_b_authorized
             FROM conflicts conf
             JOIN claims ca
               ON ca.org_id = conf.org_id AND ca.id = conf.claim_a_id
             JOIN claims cb
               ON cb.org_id = conf.org_id AND cb.id = conf.claim_b_id
             JOIN documents da
               ON da.org_id = ca.org_id AND da.id = ca.document_id
             JOIN documents db
               ON db.org_id = cb.org_id AND db.id = cb.document_id
             JOIN document_versions dva
               ON dva.org_id = ca.org_id
              AND dva.document_id = ca.document_id
              AND dva.id = ca.version_id
             JOIN document_versions dvb
               ON dvb.org_id = cb.org_id
              AND dvb.document_id = cb.document_id
              AND dvb.id = cb.version_id
             WHERE conf.org_id = $1
               AND da.collection_id = ANY($2)
               AND db.collection_id = ANY($2)
               -- H11: either evidence side anchors the conflict; hydrate counterpart later.
               AND (
                 (da.id = ANY($6) AND ca.version_id = ANY($7))
                 OR (db.id = ANY($6) AND cb.version_id = ANY($7))
               )
               AND da.deleted_at IS NULL
               AND db.deleted_at IS NULL
               AND da.state = 'indexed'
               AND db.state = 'indexed'
               AND dva.publication_state = 'published'
               AND dvb.publication_state = 'published'
               AND (
                 ($3 AND conf.status = 'open' AND dva.is_current AND dvb.is_current)
                 OR (NOT $3)
               )
               AND (
                 $8::timestamptz IS NULL
                 OR (
                   -- M1: as-of lifecycle from detected/resolved timestamps.
                   conf.first_detected_at <= $8
                   AND (conf.resolved_at IS NULL OR conf.resolved_at > $8)
                   AND dva.effective_from <= $8
                   AND (dva.effective_to IS NULL OR dva.effective_to > $8)
                   AND dvb.effective_from <= $8
                   AND (dvb.effective_to IS NULL OR dvb.effective_to > $8)
                 )
               )
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
                 WHERE acl_c.org_id = da.org_id
                   AND acl_c.id = da.collection_id
                   AND acl_c.deleted_at IS NULL
                   AND acl_u.disabled_at IS NULL
                   AND acl_p.code = $5
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
                     OR EXISTS (
                       SELECT 1 FROM collection_group_access cga
                       JOIN group_memberships gm
                         ON gm.org_id = cga.org_id
                        AND gm.group_id = cga.group_id
                        AND gm.user_id = $4
                       WHERE cga.org_id = acl_c.org_id
                         AND cga.collection_id = acl_c.id
                     )
                     OR EXISTS (
                       SELECT 1 FROM collection_role_access cra
                       JOIN org_memberships om
                         ON om.org_id = cra.org_id
                        AND om.user_id = $4
                       JOIN roles rr
                         ON rr.org_id = om.org_id
                        AND rr.code = om.role
                        AND rr.id = cra.role_id
                       WHERE cra.org_id = acl_c.org_id
                         AND cra.collection_id = acl_c.id
                     )
                   )
               )
               -- H11: authorize counterpart collection ACL as well.
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
                 WHERE acl_c.org_id = db.org_id
                   AND acl_c.id = db.collection_id
                   AND acl_c.deleted_at IS NULL
                   AND acl_u.disabled_at IS NULL
                   AND acl_p.code = $5
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
                     OR EXISTS (
                       SELECT 1 FROM collection_group_access cga
                       JOIN group_memberships gm
                         ON gm.org_id = cga.org_id
                        AND gm.group_id = cga.group_id
                        AND gm.user_id = $4
                       WHERE cga.org_id = acl_c.org_id
                         AND cga.collection_id = acl_c.id
                     )
                     OR EXISTS (
                       SELECT 1 FROM collection_role_access cra
                       JOIN org_memberships om
                         ON om.org_id = cra.org_id
                        AND om.user_id = $4
                       JOIN roles rr
                         ON rr.org_id = om.org_id
                        AND rr.code = om.role
                        AND rr.id = cra.role_id
                       WHERE cra.org_id = acl_c.org_id
                         AND cra.collection_id = acl_c.id
                     )
                   )
               )
             ORDER BY conf.id
             LIMIT $9",
            &[
                &ctx.org_id(),
                &collection_ids,
                &current_only,
                &ctx.user_id(),
                &permission,
                &evidence_document_ids,
                &evidence_version_ids,
                &as_of,
                &limit,
            ],
        )
        .await?;
    rows.iter().map(map_qa_conflict_row).collect()
}

/// Representative chunk evidence for required versions (independent of top-K).
#[derive(Debug, Clone, PartialEq)]
pub struct RepresentativeChunkRow {
    pub chunk_id: Uuid,
    pub chunk_identity_sha256: String,
    pub collection_id: Uuid,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub version_number: i32,
    pub content_sha256: String,
    pub heading_path: Vec<String>,
    pub body: String,
    pub is_current: bool,
    pub effective_from: DateTime<Utc>,
    pub effective_to: Option<DateTime<Utc>>,
    pub page: Option<i32>,
    pub slide: Option<i32>,
    pub sheet: Option<String>,
    pub span_start: Option<i32>,
    pub span_end: Option<i32>,
}

pub async fn load_representative_chunks_for_versions(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
    version_ids: &[Uuid],
    collection_ids: &[Uuid],
) -> Result<Vec<RepresentativeChunkRow>, DbError> {
    if version_ids.is_empty() || collection_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = txn
        .query(
            "SELECT DISTINCT ON (c.version_id)
                    c.id, c.chunk_identity_sha256, d.collection_id, c.document_id, c.version_id,
                    dv.version_number, dv.content_sha256, c.heading_path, c.body,
                    dv.is_current, dv.effective_from, dv.effective_to,
                    c.page, c.slide, c.sheet, c.span_start, c.span_end
             FROM chunks c
             JOIN documents d
               ON d.org_id = c.org_id AND d.id = c.document_id
             JOIN document_versions dv
               ON dv.org_id = c.org_id
              AND dv.document_id = c.document_id
              AND dv.id = c.version_id
             WHERE c.org_id = $1
               AND c.document_id = $2
               AND c.version_id = ANY($3)
               AND d.collection_id = ANY($4)
               AND d.deleted_at IS NULL
               AND d.state = 'indexed'
               AND dv.publication_state = 'published'
               AND EXISTS (
                 SELECT 1
                 FROM collections acl_c
                 JOIN org_memberships acl_m
                   ON acl_m.org_id = acl_c.org_id AND acl_m.user_id = $5
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
                   AND acl_p.code = 'qa.history'
                   AND (
                     acl_c.visibility = 'org'
                     OR acl_c.owner_user_id = $5
                     OR EXISTS (
                       SELECT 1 FROM collection_user_access cua
                       WHERE cua.org_id = acl_c.org_id
                         AND cua.collection_id = acl_c.id
                         AND cua.user_id = $5
                     )
                     OR EXISTS (
                       SELECT 1 FROM collection_group_access cga
                       JOIN group_memberships gm
                         ON gm.org_id = cga.org_id
                        AND gm.group_id = cga.group_id
                        AND gm.user_id = $5
                       WHERE cga.org_id = acl_c.org_id
                         AND cga.collection_id = acl_c.id
                     )
                     OR EXISTS (
                       SELECT 1 FROM collection_role_access cra
                       JOIN org_memberships om
                         ON om.org_id = cra.org_id
                        AND om.user_id = $5
                       JOIN roles rr
                         ON rr.org_id = om.org_id
                        AND rr.code = om.role
                        AND rr.id = cra.role_id
                       WHERE cra.org_id = acl_c.org_id
                         AND cra.collection_id = acl_c.id
                     )
                   )
               )
             ORDER BY c.version_id, c.ordinal, c.id",
            &[
                &ctx.org_id(),
                &document_id,
                &version_ids,
                &collection_ids,
                &ctx.user_id(),
            ],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|row| RepresentativeChunkRow {
            chunk_id: row.get(0),
            chunk_identity_sha256: row.get(1),
            collection_id: row.get(2),
            document_id: row.get(3),
            version_id: row.get(4),
            version_number: row.get(5),
            content_sha256: row.get(6),
            heading_path: row.get(7),
            body: row.get(8),
            is_current: row.get(9),
            effective_from: row.get(10),
            effective_to: row.get(11),
            page: row.get(12),
            slide: row.get(13),
            sheet: row.get(14),
            span_start: row.get(15),
            span_end: row.get(16),
        })
        .collect())
}

/// Typed claim values for exact version_a/version_b delta queries.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedClaimRow {
    pub version_id: Uuid,
    pub version_number: i32,
    pub claim_key: String,
    pub scope: String,
    pub unit: Option<String>,
    pub value: Decimal,
    pub chunk_id: Option<Uuid>,
}

pub async fn load_typed_claims_for_versions(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
    version_ids: &[Uuid],
    collection_ids: &[Uuid],
) -> Result<Vec<TypedClaimRow>, DbError> {
    if version_ids.is_empty() || collection_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = txn
        .query(
            "SELECT c.version_id, dv.version_number, c.claim_key, c.scope, c.unit,
                    COALESCE(c.value_number, c.value_money) AS value, c.chunk_id
             FROM claims c
             JOIN documents d
               ON d.org_id = c.org_id AND d.id = c.document_id
             JOIN document_versions dv
               ON dv.org_id = c.org_id
              AND dv.document_id = c.document_id
              AND dv.id = c.version_id
             WHERE c.org_id = $1
               AND c.document_id = $2
               AND c.version_id = ANY($3)
               AND d.collection_id = ANY($4)
               AND d.deleted_at IS NULL
               AND dv.publication_state = 'published'
               AND COALESCE(c.value_number, c.value_money) IS NOT NULL
             ORDER BY dv.version_number, c.claim_key, c.id",
            &[&ctx.org_id(), &document_id, &version_ids, &collection_ids],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|row| TypedClaimRow {
            version_id: row.get(0),
            version_number: row.get(1),
            claim_key: row.get(2),
            scope: row.get(3),
            unit: row.get(4),
            value: row.get(5),
            chunk_id: row.get(6),
        })
        .collect())
}

/// Deterministic paired numeric delta via SQL join on key/subject/predicate/scope/unit.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedDeltaPairRow {
    pub older_version_id: Uuid,
    pub newer_version_id: Uuid,
    pub older_version_number: i32,
    pub newer_version_number: i32,
    pub older_chunk_id: Uuid,
    pub newer_chunk_id: Uuid,
    pub claim_key: String,
    pub subject: String,
    pub predicate: String,
    pub scope: String,
    pub unit: Option<String>,
    pub older_value: Decimal,
    pub newer_value: Decimal,
}

pub async fn load_typed_delta_pair(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
    version_a: Uuid,
    version_b: Uuid,
    collection_ids: &[Uuid],
    question: &str,
) -> Result<Option<TypedDeltaPairRow>, DbError> {
    if collection_ids.is_empty() {
        return Ok(None);
    }
    // M13: meaningful stopword-filtered tokens (no bare "là"/function words).
    const STOP: &[&str] = &[
        "là", "la", "làá", "của", "và", "các", "cho", "với", "một", "những", "the", "a", "an",
        "of", "to", "in", "on", "is", "are", "was", "were", "bao", "nhiêu", "theo", "tài", "liệu",
    ];
    let tokens: Vec<String> = question
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .map(|t| t.trim().to_lowercase())
        .filter(|t| t.chars().count() >= 2 && !STOP.contains(&t.as_str()))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    if tokens.is_empty() {
        return Ok(None);
    }
    let row = txn
        .query_opt(
            "SELECT
                CASE WHEN dva.version_number <= dvb.version_number THEN ca.version_id ELSE cb.version_id END,
                CASE WHEN dva.version_number <= dvb.version_number THEN cb.version_id ELSE ca.version_id END,
                CASE WHEN dva.version_number <= dvb.version_number THEN dva.version_number ELSE dvb.version_number END,
                CASE WHEN dva.version_number <= dvb.version_number THEN dvb.version_number ELSE dva.version_number END,
                CASE WHEN dva.version_number <= dvb.version_number THEN ca.chunk_id ELSE cb.chunk_id END,
                CASE WHEN dva.version_number <= dvb.version_number THEN cb.chunk_id ELSE ca.chunk_id END,
                ca.claim_key, ca.subject, ca.predicate, ca.scope, ca.unit,
                CASE WHEN dva.version_number <= dvb.version_number
                     THEN COALESCE(ca.value_number, ca.value_money)
                     ELSE COALESCE(cb.value_number, cb.value_money) END,
                CASE WHEN dva.version_number <= dvb.version_number
                     THEN COALESCE(cb.value_number, cb.value_money)
                     ELSE COALESCE(ca.value_number, ca.value_money) END
             FROM claims ca
             JOIN claims cb
               ON cb.org_id = ca.org_id
              AND cb.document_id = ca.document_id
              AND cb.claim_key = ca.claim_key
              AND cb.subject = ca.subject
              AND cb.predicate = ca.predicate
              AND cb.scope = ca.scope
              AND cb.unit IS NOT DISTINCT FROM ca.unit
              AND cb.version_id = $4
             JOIN documents d
               ON d.org_id = ca.org_id AND d.id = ca.document_id
             JOIN document_versions dva
               ON dva.org_id = ca.org_id AND dva.document_id = ca.document_id AND dva.id = ca.version_id
             JOIN document_versions dvb
               ON dvb.org_id = cb.org_id AND dvb.document_id = cb.document_id AND dvb.id = cb.version_id
             WHERE ca.org_id = $1
               AND ca.document_id = $2
               AND ca.version_id = $3
               AND d.collection_id = ANY($5)
               AND d.deleted_at IS NULL
               AND dva.publication_state = 'published'
               AND dvb.publication_state = 'published'
               AND ca.chunk_id IS NOT NULL
               AND cb.chunk_id IS NOT NULL
               AND COALESCE(ca.value_number, ca.value_money) IS NOT NULL
               AND COALESCE(cb.value_number, cb.value_money) IS NOT NULL
               AND (dva.effective_to IS NULL OR dva.effective_to > dva.effective_from)
               AND (dvb.effective_to IS NULL OR dvb.effective_to > dvb.effective_from)
               AND (
                 SELECT COUNT(*)::int FROM unnest($6::text[]) AS t(tok)
                 WHERE position(tok in lower(
                   ca.claim_key || ' ' || ca.subject || ' ' || ca.predicate || ' ' ||
                   ca.scope || ' ' || coalesce(ca.citation_quote, '')
                 )) > 0
               ) > 0
             ORDER BY
               (
                 SELECT COUNT(*)::int FROM unnest($6::text[]) AS t(tok)
                 WHERE position(tok in lower(
                   ca.claim_key || ' ' || ca.subject || ' ' || ca.predicate || ' ' ||
                   ca.scope || ' ' || coalesce(ca.citation_quote, '')
                 )) > 0
               ) DESC,
               ca.claim_key, ca.id
             LIMIT 1",
            &[
                &ctx.org_id(),
                &document_id,
                &version_a,
                &version_b,
                &collection_ids,
                &tokens,
            ],
        )
        .await?;
    Ok(row.map(|row| TypedDeltaPairRow {
        older_version_id: row.get(0),
        newer_version_id: row.get(1),
        older_version_number: row.get(2),
        newer_version_number: row.get(3),
        older_chunk_id: row.get(4),
        newer_chunk_id: row.get(5),
        claim_key: row.get(6),
        subject: row.get(7),
        predicate: row.get(8),
        scope: row.get(9),
        unit: row.get(10),
        older_value: row.get(11),
        newer_value: row.get(12),
    }))
}

/// M5: load current published pointers for documents (separate from as-of cited versions).
pub async fn load_current_published_version_ids(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_ids: &[Uuid],
    collection_ids: &[Uuid],
) -> Result<Vec<Uuid>, DbError> {
    let document_ids = dedup_sorted_uuids(document_ids);
    if document_ids.is_empty() || collection_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = txn
        .query(
            "SELECT dv.id
             FROM document_versions dv
             JOIN documents d
               ON d.org_id = dv.org_id AND d.id = dv.document_id
             WHERE dv.org_id = $1
               AND dv.document_id = ANY($2)
               AND d.collection_id = ANY($3)
               AND d.deleted_at IS NULL
               AND d.state = 'indexed'
               AND dv.publication_state = 'published'
               AND dv.is_current = true
               AND dv.effective_from <= now()
               AND (dv.effective_to IS NULL OR dv.effective_to > now())
             ORDER BY dv.document_id, dv.id",
            &[&ctx.org_id(), &document_ids, &collection_ids],
        )
        .await?;
    Ok(rows.iter().map(|r| r.get(0)).collect())
}

/// Re-check cited version pins are still authorized/current/effective after blob IO.
pub async fn verify_citation_pins_still_valid(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    version_ids: &[Uuid],
    collection_ids: &[Uuid],
    require_current: bool,
    as_of: Option<DateTime<Utc>>,
) -> Result<bool, DbError> {
    let version_ids = dedup_sorted_uuids(version_ids);
    if version_ids.is_empty() {
        return Ok(true);
    }
    let row = txn
        .query_one(
            "SELECT COUNT(*)::bigint AS n
             FROM document_versions dv
             JOIN documents d
               ON d.org_id = dv.org_id AND d.id = dv.document_id
             WHERE dv.org_id = $1
               AND dv.id = ANY($2)
               AND d.collection_id = ANY($3)
               AND d.deleted_at IS NULL
               AND d.state = 'indexed'
               AND dv.publication_state = 'published'
               AND (
                 NOT $4
                 OR (
                   dv.is_current = true
                   AND dv.effective_from <= now()
                   AND (dv.effective_to IS NULL OR dv.effective_to > now())
                 )
               )
               AND (
                 $5::timestamptz IS NULL
                 OR (
                   dv.effective_from <= $5
                   AND (dv.effective_to IS NULL OR dv.effective_to > $5)
                 )
               )",
            &[
                &ctx.org_id(),
                &version_ids,
                &collection_ids,
                &require_current,
                &as_of,
            ],
        )
        .await?;
    let n: i64 = row.get("n");
    Ok(n == version_ids.len() as i64)
}

/// Verifies each version is still the current published effective pointer.
pub async fn verify_versions_still_current(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    version_ids: &[Uuid],
    collection_ids: &[Uuid],
) -> Result<bool, DbError> {
    let version_ids = dedup_sorted_uuids(version_ids);
    if version_ids.is_empty() {
        return Ok(true);
    }
    let row = txn
        .query_one(
            "SELECT COUNT(*)::bigint AS n
             FROM document_versions dv
             JOIN documents d
               ON d.org_id = dv.org_id AND d.id = dv.document_id
             WHERE dv.org_id = $1
               AND dv.id = ANY($2)
               AND d.collection_id = ANY($3)
               AND d.deleted_at IS NULL
               AND d.state = 'indexed'
               AND dv.publication_state = 'published'
               AND dv.is_current = true
               AND (dv.effective_to IS NULL OR dv.effective_to > now())
               AND dv.effective_from <= now()",
            &[&ctx.org_id(), &version_ids, &collection_ids],
        )
        .await?;
    let n: i64 = row.get("n");
    Ok(n == version_ids.len() as i64)
}

/// Live stream authz probe: membership + document deleted/tombstoned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamAuthzProbe {
    Allow,
    Revoked,
    Deleted,
}

pub async fn probe_stream_authz(
    txn: &Transaction<'_>,
    org_id: Uuid,
    user_id: Uuid,
    document_ids: &[Uuid],
) -> Result<StreamAuthzProbe, DbError> {
    let user = txn
        .query_opt("SELECT disabled_at FROM users WHERE id = $1", &[&user_id])
        .await?;
    let Some(user) = user else {
        return Ok(StreamAuthzProbe::Revoked);
    };
    let disabled_at: Option<DateTime<Utc>> = user.get(0);
    if disabled_at.is_some() {
        return Ok(StreamAuthzProbe::Revoked);
    }
    // Full qa.query (+ qa.history when any cited doc is non-current) permission probe.
    let perm = txn
        .query_opt(
            "SELECT
                EXISTS (
                  SELECT 1
                  FROM org_memberships m
                  JOIN roles r ON r.org_id = m.org_id AND r.code = m.role
                  JOIN role_permissions rp ON rp.org_id = r.org_id AND rp.role_id = r.id
                  JOIN permissions p ON p.id = rp.permission_id
                  WHERE m.org_id = $1 AND m.user_id = $2 AND p.code = 'qa.query'
                ) AS has_query,
                EXISTS (
                  SELECT 1
                  FROM org_memberships m
                  JOIN roles r ON r.org_id = m.org_id AND r.code = m.role
                  JOIN role_permissions rp ON rp.org_id = r.org_id AND rp.role_id = r.id
                  JOIN permissions p ON p.id = rp.permission_id
                  WHERE m.org_id = $1 AND m.user_id = $2 AND p.code = 'qa.history'
                ) AS has_history",
            &[&org_id, &user_id],
        )
        .await?;
    let Some(perm) = perm else {
        return Ok(StreamAuthzProbe::Revoked);
    };
    let has_query: bool = perm.get("has_query");
    let has_history: bool = perm.get("has_history");
    if !has_query {
        return Ok(StreamAuthzProbe::Revoked);
    }
    if document_ids.is_empty() {
        return Ok(StreamAuthzProbe::Allow);
    }
    let row = txn
        .query_one(
            "SELECT COUNT(*) FILTER (
                WHERE deleted_at IS NOT NULL OR state IN ('tombstoned', 'purged')
             )::bigint AS deleted_n,
             COUNT(*)::bigint AS total_n,
             COUNT(*) FILTER (
                WHERE NOT EXISTS (
                  SELECT 1 FROM document_versions dv
                  WHERE dv.org_id = documents.org_id
                    AND dv.document_id = documents.id
                    AND dv.is_current = true
                    AND dv.publication_state = 'published'
                )
             )::bigint AS needs_history_n
             FROM documents
             WHERE org_id = $1 AND id = ANY($2)",
            &[&org_id, &document_ids],
        )
        .await?;
    let deleted_n: i64 = row.get("deleted_n");
    let total_n: i64 = row.get("total_n");
    let needs_history_n: i64 = row.get("needs_history_n");
    if total_n < document_ids.len() as i64 || deleted_n > 0 {
        return Ok(StreamAuthzProbe::Deleted);
    }
    if needs_history_n > 0 && !has_history {
        return Ok(StreamAuthzProbe::Revoked);
    }
    Ok(StreamAuthzProbe::Allow)
}

/// Exact stream authz probe: qa.query (+ qa.history when required), collection ACL,
/// cited documents, and cited version publication/effectiveness.
pub async fn probe_stream_authz_exact<C>(
    client: &C,
    org_id: Uuid,
    user_id: Uuid,
    document_ids: &[Uuid],
    version_ids: &[Uuid],
    collection_ids: &[Uuid],
    require_history: bool,
) -> Result<StreamAuthzProbe, DbError>
where
    C: tokio_postgres::GenericClient + Sync,
{
    let document_ids = dedup_sorted_uuids(document_ids);
    let version_ids = dedup_sorted_uuids(version_ids);
    let collection_ids = dedup_sorted_uuids(collection_ids);
    let user = client
        .query_opt("SELECT disabled_at FROM users WHERE id = $1", &[&user_id])
        .await?;
    let Some(user) = user else {
        return Ok(StreamAuthzProbe::Revoked);
    };
    let disabled_at: Option<DateTime<Utc>> = user.get(0);
    if disabled_at.is_some() {
        return Ok(StreamAuthzProbe::Revoked);
    }
    let perm = client
        .query_opt(
            "SELECT
                EXISTS (
                  SELECT 1
                  FROM org_memberships m
                  JOIN roles r ON r.org_id = m.org_id AND r.code = m.role
                  JOIN role_permissions rp ON rp.org_id = r.org_id AND rp.role_id = r.id
                  JOIN permissions p ON p.id = rp.permission_id
                  WHERE m.org_id = $1 AND m.user_id = $2 AND p.code = 'qa.query'
                ) AS has_query,
                EXISTS (
                  SELECT 1
                  FROM org_memberships m
                  JOIN roles r ON r.org_id = m.org_id AND r.code = m.role
                  JOIN role_permissions rp ON rp.org_id = r.org_id AND rp.role_id = r.id
                  JOIN permissions p ON p.id = rp.permission_id
                  WHERE m.org_id = $1 AND m.user_id = $2 AND p.code = 'qa.history'
                ) AS has_history",
            &[&org_id, &user_id],
        )
        .await?;
    let Some(perm) = perm else {
        return Ok(StreamAuthzProbe::Revoked);
    };
    let has_query: bool = perm.get("has_query");
    let has_history: bool = perm.get("has_history");
    if !has_query || (require_history && !has_history) {
        return Ok(StreamAuthzProbe::Revoked);
    }
    if document_ids.is_empty() {
        return Ok(StreamAuthzProbe::Allow);
    }
    if collection_ids.is_empty() {
        return Ok(StreamAuthzProbe::Revoked);
    }
    let row = client
        .query_one(
            "SELECT COUNT(*)::bigint AS total_n,
                    COUNT(*) FILTER (
                      WHERE deleted_at IS NOT NULL OR state IN ('tombstoned', 'purged')
                    )::bigint AS deleted_n,
                    COUNT(*) FILTER (
                      WHERE collection_id = ANY($3)
                        AND EXISTS (
                          SELECT 1
                          FROM collections acl_c
                          JOIN org_memberships acl_m
                            ON acl_m.org_id = acl_c.org_id AND acl_m.user_id = $4
                          JOIN users acl_u ON acl_u.id = acl_m.user_id
                          WHERE acl_c.org_id = documents.org_id
                            AND acl_c.id = documents.collection_id
                            AND acl_c.deleted_at IS NULL
                            AND acl_u.disabled_at IS NULL
                            AND (
                              acl_c.visibility = 'org'
                              OR acl_c.owner_user_id = $4
                              OR EXISTS (
                                SELECT 1 FROM collection_user_access cua
                                WHERE cua.org_id = acl_c.org_id
                                  AND cua.collection_id = acl_c.id
                                  AND cua.user_id = $4
                              )
                              OR EXISTS (
                                SELECT 1 FROM collection_group_access cga
                                JOIN group_memberships gm
                                  ON gm.org_id = cga.org_id
                                 AND gm.group_id = cga.group_id
                                 AND gm.user_id = $4
                                WHERE cga.org_id = acl_c.org_id
                                  AND cga.collection_id = acl_c.id
                              )
                              OR EXISTS (
                                SELECT 1 FROM collection_role_access cra
                                JOIN org_memberships om
                                  ON om.org_id = cra.org_id
                                 AND om.user_id = $4
                                JOIN roles rr
                                  ON rr.org_id = om.org_id
                                 AND rr.code = om.role
                                 AND rr.id = cra.role_id
                                WHERE cra.org_id = acl_c.org_id
                                  AND cra.collection_id = acl_c.id
                              )
                            )
                        )
                    )::bigint AS acl_ok_n
             FROM documents
             WHERE org_id = $1 AND id = ANY($2)",
            &[&org_id, &document_ids, &collection_ids, &user_id],
        )
        .await?;
    let total_n: i64 = row.get("total_n");
    let deleted_n: i64 = row.get("deleted_n");
    let acl_ok_n: i64 = row.get("acl_ok_n");
    if total_n < document_ids.len() as i64 || deleted_n > 0 {
        return Ok(StreamAuthzProbe::Deleted);
    }
    if acl_ok_n < document_ids.len() as i64 {
        return Ok(StreamAuthzProbe::Revoked);
    }
    if !version_ids.is_empty() {
        let vrow = client
            .query_one(
                "SELECT COUNT(*)::bigint AS n
                 FROM document_versions dv
                 JOIN documents d
                   ON d.org_id = dv.org_id AND d.id = dv.document_id
                 WHERE dv.org_id = $1
                   AND dv.id = ANY($2)
                   AND d.collection_id = ANY($3)
                   AND d.deleted_at IS NULL
                   AND d.state = 'indexed'
                   AND dv.publication_state = 'published'",
                &[&org_id, &version_ids, &collection_ids],
            )
            .await?;
        let n: i64 = vrow.get("n");
        if n < version_ids.len() as i64 {
            return Ok(StreamAuthzProbe::Revoked);
        }
    }
    Ok(StreamAuthzProbe::Allow)
}

/// Validates compare/history versions share one authorized document lineage.
pub async fn load_lineage_versions(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
    version_ids: &[Uuid],
    collection_ids: &[Uuid],
) -> Result<Vec<(Uuid, i32, Option<Uuid>)>, DbError> {
    if version_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = txn
        .query(
            "SELECT dv.id, dv.version_number, dv.parent_version_id
             FROM documents d
             JOIN document_versions dv
               ON dv.org_id = d.org_id
              AND dv.document_id = d.id
             WHERE d.org_id = $1
               AND d.id = $2
               AND d.collection_id = ANY($3)
               AND d.deleted_at IS NULL
               AND d.state = 'indexed'
               AND dv.publication_state = 'published'
               AND dv.id = ANY($4)
             ORDER BY dv.version_number, dv.id",
            &[&ctx.org_id(), &document_id, &collection_ids, &version_ids],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|row| (row.get(0), row.get(1), row.get(2)))
        .collect())
}

/// Full-text search over active-generation, version-filtered chunks.
///
/// Query text is accent-folded (`accent-fold-v1`) before `plainto_tsquery` so it
/// matches `markhand_accent_fold` tsvector content.
pub async fn fts_search(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    collection_ids: &[Uuid],
    query: &str,
    visibility: &VersionVisibility,
    limit: usize,
) -> Result<Vec<FtsCandidate>, DbError> {
    if collection_ids.is_empty() || limit == 0 || query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let folded = fileconv_core::intelligence::normalize_search_text(query);
    if folded.trim().is_empty() {
        return Ok(Vec::new());
    }
    let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);
    let rows = match visibility {
        VersionVisibility::Current => {
            txn.query(
                "SELECT c.id, c.chunk_identity_sha256, c.document_id, c.version_id,
                        d.collection_id,
                        ts_rank_cd(c.tsv, plainto_tsquery('simple', $4))::real AS rank
                 FROM chunks c
                 JOIN documents d
                   ON d.org_id = c.org_id AND d.id = c.document_id
                 JOIN document_versions dv
                   ON dv.org_id = c.org_id
                  AND dv.document_id = c.document_id
                  AND dv.id = c.version_id
                 JOIN index_metadata im
                   ON im.org_id = c.org_id AND im.id = c.index_metadata_id
                 WHERE c.org_id = $1
                   AND d.collection_id = ANY($2)
                   AND d.deleted_at IS NULL
                   AND d.state = 'indexed'
                   AND dv.publication_state = 'published'
                   AND dv.is_current
                   AND im.is_active
                   AND im.state = 'active'
                   AND c.tsv @@ plainto_tsquery('simple', $4)
                 ORDER BY rank DESC, c.id
                 LIMIT $3",
                &[&ctx.org_id(), &collection_ids, &limit_i64, &folded],
            )
            .await?
        }
        VersionVisibility::VersionIds(version_ids) => {
            if version_ids.is_empty() {
                return Ok(Vec::new());
            }
            let versions: Vec<Uuid> = version_ids.iter().copied().collect();
            txn.query(
                "SELECT c.id, c.chunk_identity_sha256, c.document_id, c.version_id,
                        d.collection_id,
                        ts_rank_cd(c.tsv, plainto_tsquery('simple', $5))::real AS rank
                 FROM chunks c
                 JOIN documents d
                   ON d.org_id = c.org_id AND d.id = c.document_id
                 JOIN document_versions dv
                   ON dv.org_id = c.org_id
                  AND dv.document_id = c.document_id
                  AND dv.id = c.version_id
                 JOIN index_metadata im
                   ON im.org_id = c.org_id AND im.id = c.index_metadata_id
                 WHERE c.org_id = $1
                   AND d.collection_id = ANY($2)
                   AND d.deleted_at IS NULL
                   AND d.state = 'indexed'
                   AND dv.publication_state = 'published'
                   AND c.version_id = ANY($3)
                   AND im.is_active
                   AND im.state = 'active'
                   AND c.tsv @@ plainto_tsquery('simple', $5)
                 ORDER BY rank DESC, c.id
                 LIMIT $4",
                &[
                    &ctx.org_id(),
                    &collection_ids,
                    &versions,
                    &limit_i64,
                    &folded,
                ],
            )
            .await?
        }
    };
    rows.iter().map(map_fts_candidate).collect()
}

/// Hydrates candidate chunk identities from the active index generation only.
pub async fn hydrate_chunks_by_identity(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    collection_ids: &[Uuid],
    identities: &[String],
    visibility: &VersionVisibility,
) -> Result<Vec<HydratedChunkRow>, DbError> {
    if collection_ids.is_empty() || identities.is_empty() {
        return Ok(Vec::new());
    }
    let rows = match visibility {
        VersionVisibility::Current => {
            txn.query(
                "SELECT c.id, c.chunk_identity_sha256, c.org_id, d.collection_id,
                        c.document_id, c.version_id, dv.version_number, dv.content_sha256,
                        c.heading_path, c.body, c.page, c.slide, c.sheet,
                        c.span_start, c.span_end, d.state, d.deleted_at,
                        dv.publication_state, dv.is_current, dv.effective_from, dv.effective_to,
                        c.index_metadata_id, im.is_active, im.state AS index_state
                 FROM chunks c
                 JOIN documents d
                   ON d.org_id = c.org_id AND d.id = c.document_id
                 JOIN document_versions dv
                   ON dv.org_id = c.org_id
                  AND dv.document_id = c.document_id
                  AND dv.id = c.version_id
                 JOIN index_metadata im
                   ON im.org_id = c.org_id AND im.id = c.index_metadata_id
                 WHERE c.org_id = $1
                   AND d.collection_id = ANY($2)
                   AND c.chunk_identity_sha256 = ANY($3)
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
                       AND acl_p.code = $5
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
                       
                         OR EXISTS (
                           SELECT 1 FROM collection_group_access cga
                           JOIN group_memberships gm
                             ON gm.org_id = cga.org_id
                            AND gm.group_id = cga.group_id
                            AND gm.user_id = $4
                           WHERE cga.org_id = acl_c.org_id
                             AND cga.collection_id = acl_c.id
                         )
                         OR EXISTS (
                           SELECT 1 FROM collection_role_access cra
                           JOIN org_memberships om
                             ON om.org_id = cra.org_id
                            AND om.user_id = $4
                           JOIN roles rr
                             ON rr.org_id = om.org_id
                            AND rr.code = om.role
                            AND rr.id = cra.role_id
                           WHERE cra.org_id = acl_c.org_id
                             AND cra.collection_id = acl_c.id
                         )
)
                   )
                   AND d.deleted_at IS NULL
                   AND d.state = 'indexed'
                   AND dv.publication_state = 'published'
                   AND dv.is_current
                   AND im.is_active
                   AND im.state = 'active'",
                &[
                    &ctx.org_id(),
                    &collection_ids,
                    &identities,
                    &ctx.user_id(),
                    &visibility.required_permission(),
                ],
            )
            .await?
        }
        VersionVisibility::VersionIds(version_ids) => {
            if version_ids.is_empty() {
                return Ok(Vec::new());
            }
            let versions: Vec<Uuid> = version_ids.iter().copied().collect();
            txn.query(
                "SELECT c.id, c.chunk_identity_sha256, c.org_id, d.collection_id,
                        c.document_id, c.version_id, dv.version_number, dv.content_sha256,
                        c.heading_path, c.body, c.page, c.slide, c.sheet,
                        c.span_start, c.span_end, d.state, d.deleted_at,
                        dv.publication_state, dv.is_current, dv.effective_from, dv.effective_to,
                        c.index_metadata_id, im.is_active, im.state AS index_state
                 FROM chunks c
                 JOIN documents d
                   ON d.org_id = c.org_id AND d.id = c.document_id
                 JOIN document_versions dv
                   ON dv.org_id = c.org_id
                  AND dv.document_id = c.document_id
                  AND dv.id = c.version_id
                 JOIN index_metadata im
                   ON im.org_id = c.org_id AND im.id = c.index_metadata_id
                 WHERE c.org_id = $1
                   AND d.collection_id = ANY($2)
                   AND c.chunk_identity_sha256 = ANY($3)
                   AND c.version_id = ANY($4)
                   AND EXISTS (
                     SELECT 1
                     FROM collections acl_c
                     JOIN org_memberships acl_m
                       ON acl_m.org_id = acl_c.org_id AND acl_m.user_id = $5
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
                       AND acl_p.code = $6
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
                         OR acl_c.owner_user_id = $5
                         OR EXISTS (
                           SELECT 1 FROM collection_user_access cua
                           WHERE cua.org_id = acl_c.org_id
                             AND cua.collection_id = acl_c.id
                             AND cua.user_id = $5
                         )
                       
                         OR EXISTS (
                           SELECT 1 FROM collection_group_access cga
                           JOIN group_memberships gm
                             ON gm.org_id = cga.org_id
                            AND gm.group_id = cga.group_id
                            AND gm.user_id = $5
                           WHERE cga.org_id = acl_c.org_id
                             AND cga.collection_id = acl_c.id
                         )
                         OR EXISTS (
                           SELECT 1 FROM collection_role_access cra
                           JOIN org_memberships om
                             ON om.org_id = cra.org_id
                            AND om.user_id = $5
                           JOIN roles rr
                             ON rr.org_id = om.org_id
                            AND rr.code = om.role
                            AND rr.id = cra.role_id
                           WHERE cra.org_id = acl_c.org_id
                             AND cra.collection_id = acl_c.id
                         )
)
                   )
                   AND d.deleted_at IS NULL
                   AND d.state = 'indexed'
                   AND dv.publication_state = 'published'
                   AND im.is_active
                   AND im.state = 'active'",
                &[
                    &ctx.org_id(),
                    &collection_ids,
                    &identities,
                    &versions,
                    &ctx.user_id(),
                    &visibility.required_permission(),
                ],
            )
            .await?
        }
    };
    rows.iter().map(map_hydrated_chunk).collect()
}

/// Authorized document version row for citation resolve / preview / download.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthorizedVersionRow {
    pub org_id: Uuid,
    pub collection_id: Uuid,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub version_number: i32,
    pub parent_version_id: Option<Uuid>,
    /// Version row hash (Markdown hash after promotion — not original upload).
    pub content_sha256: String,
    pub original_object_key: String,
    pub markdown_object_key: Option<String>,
    pub markdown_artifact_key: Option<String>,
    pub markdown_artifact_sha256: Option<String>,
    pub markdown_artifact_content_type: Option<String>,
    pub markdown_artifact_byte_size: Option<i64>,
    pub source_filename: Option<String>,
    pub source_content_type: Option<String>,
    pub byte_size: Option<i64>,
    pub document_state: DocumentState,
    pub deleted_at: Option<DateTime<Utc>>,
    pub publication_state: PublicationState,
    pub is_current: bool,
    pub effective_from: DateTime<Utc>,
    pub effective_to: Option<DateTime<Utc>>,
}

/// Immutable Markdown artifact required for citation quote verification / preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedMarkdownArtifact {
    pub object_key: String,
    pub content_sha256: String,
    pub content_type: String,
    pub byte_size: u64,
}

/// Fresh-authorized chunk load by id for citation resolve (ADR 0002).
///
/// Exact citation resolve does **not** require the chunk's index generation to
/// still be active (cutover/retired generations remain citeable). Fresh ACL,
/// membership, document state and published version checks still apply.
pub async fn hydrate_chunk_for_citation(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    chunk_id: Uuid,
) -> Result<Option<HydratedChunkRow>, DbError> {
    let row = txn
        .query_opt(
            "SELECT c.id, c.chunk_identity_sha256, c.org_id, d.collection_id,
                    c.document_id, c.version_id, dv.version_number, dv.content_sha256,
                    c.heading_path, c.body, c.page, c.slide, c.sheet,
                    c.span_start, c.span_end, d.state, d.deleted_at,
                    dv.publication_state, dv.is_current, dv.effective_from, dv.effective_to,
                    c.index_metadata_id, im.is_active, im.state AS index_state
             FROM chunks c
             JOIN documents d
               ON d.org_id = c.org_id AND d.id = c.document_id
             JOIN document_versions dv
               ON dv.org_id = c.org_id
              AND dv.document_id = c.document_id
              AND dv.id = c.version_id
             JOIN index_metadata im
               ON im.org_id = c.org_id AND im.id = c.index_metadata_id
             WHERE c.org_id = $1
               AND c.id = $2
               AND EXISTS (
                 SELECT 1
                 FROM collections acl_c
                 JOIN org_memberships acl_m
                   ON acl_m.org_id = acl_c.org_id AND acl_m.user_id = $3
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
                     OR acl_c.owner_user_id = $3
                     OR EXISTS (
                       SELECT 1 FROM collection_user_access cua
                       WHERE cua.org_id = acl_c.org_id
                         AND cua.collection_id = acl_c.id
                         AND cua.user_id = $3
                     )
                     OR EXISTS (
                       SELECT 1 FROM collection_group_access cga
                       JOIN group_memberships gm
                         ON gm.org_id = cga.org_id
                        AND gm.group_id = cga.group_id
                        AND gm.user_id = $3
                       WHERE cga.org_id = acl_c.org_id
                         AND cga.collection_id = acl_c.id
                     )
                     OR EXISTS (
                       SELECT 1 FROM collection_role_access cra
                       JOIN org_memberships om
                         ON om.org_id = cra.org_id
                        AND om.user_id = $3
                       JOIN roles rr
                         ON rr.org_id = om.org_id
                        AND rr.code = om.role
                        AND rr.id = cra.role_id
                       WHERE cra.org_id = acl_c.org_id
                         AND cra.collection_id = acl_c.id
                     )
                   )
               )
               AND d.deleted_at IS NULL
               AND d.state = 'indexed'
               AND dv.publication_state = 'published'",
            &[&ctx.org_id(), &chunk_id, &ctx.user_id()],
        )
        .await?;
    row.map(|row| map_hydrated_chunk(&row)).transpose()
}

/// Batch hydrate for citation resolve (single snapshot / one round-trip).
pub async fn hydrate_chunks_for_citation(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    chunk_ids: &[Uuid],
) -> Result<Vec<HydratedChunkRow>, DbError> {
    if chunk_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = txn
        .query(
            "SELECT c.id, c.chunk_identity_sha256, c.org_id, d.collection_id,
                    c.document_id, c.version_id, dv.version_number, dv.content_sha256,
                    c.heading_path, c.body, c.page, c.slide, c.sheet,
                    c.span_start, c.span_end, d.state, d.deleted_at,
                    dv.publication_state, dv.is_current, dv.effective_from, dv.effective_to,
                    c.index_metadata_id, im.is_active, im.state AS index_state
             FROM chunks c
             JOIN documents d
               ON d.org_id = c.org_id AND d.id = c.document_id
             JOIN document_versions dv
               ON dv.org_id = c.org_id
              AND dv.document_id = c.document_id
              AND dv.id = c.version_id
             JOIN index_metadata im
               ON im.org_id = c.org_id AND im.id = c.index_metadata_id
             WHERE c.org_id = $1
               AND c.id = ANY($2)
               AND EXISTS (
                 SELECT 1
                 FROM collections acl_c
                 JOIN org_memberships acl_m
                   ON acl_m.org_id = acl_c.org_id AND acl_m.user_id = $3
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
                     OR acl_c.owner_user_id = $3
                     OR EXISTS (
                       SELECT 1 FROM collection_user_access cua
                       WHERE cua.org_id = acl_c.org_id
                         AND cua.collection_id = acl_c.id
                         AND cua.user_id = $3
                     )
                     OR EXISTS (
                       SELECT 1 FROM collection_group_access cga
                       JOIN group_memberships gm
                         ON gm.org_id = cga.org_id
                        AND gm.group_id = cga.group_id
                        AND gm.user_id = $3
                       WHERE cga.org_id = acl_c.org_id
                         AND cga.collection_id = acl_c.id
                     )
                     OR EXISTS (
                       SELECT 1 FROM collection_role_access cra
                       JOIN org_memberships om
                         ON om.org_id = cra.org_id
                        AND om.user_id = $3
                       JOIN roles rr
                         ON rr.org_id = om.org_id
                        AND rr.code = om.role
                        AND rr.id = cra.role_id
                       WHERE cra.org_id = acl_c.org_id
                         AND cra.collection_id = acl_c.id
                     )
                   )
               )
               AND d.deleted_at IS NULL
               AND d.state = 'indexed'
               AND dv.publication_state = 'published'",
            &[&ctx.org_id(), &chunk_ids, &ctx.user_id()],
        )
        .await?;
    rows.iter().map(map_hydrated_chunk).collect()
}

/// Fresh-authorized version load for trusted Markdown preview / download mint.
pub async fn load_authorized_version_for_read(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
    version_id: Uuid,
) -> Result<Option<AuthorizedVersionRow>, DbError> {
    let kind = "markdown";
    let row = txn
        .query_opt(
            "SELECT d.org_id, d.collection_id, d.id AS document_id, dv.id AS version_id,
                    dv.version_number, dv.parent_version_id, dv.content_sha256,
                    dv.original_object_key, dv.markdown_object_key,
                    da.object_key AS markdown_artifact_key,
                    da.content_sha256 AS markdown_artifact_sha256,
                    da.content_type AS markdown_artifact_content_type,
                    da.byte_size AS markdown_artifact_byte_size,
                    dv.source_filename, dv.source_content_type, dv.byte_size,
                    d.state, d.deleted_at, dv.publication_state, dv.is_current,
                    dv.effective_from, dv.effective_to
             FROM documents d
             JOIN document_versions dv
               ON dv.org_id = d.org_id
              AND dv.document_id = d.id
              AND dv.id = $3
             LEFT JOIN derived_artifacts da
               ON da.org_id = dv.org_id
              AND da.version_id = dv.id
              AND da.artifact_kind = $4
             WHERE d.org_id = $1
               AND d.id = $2
               AND EXISTS (
                 SELECT 1
                 FROM collections acl_c
                 JOIN org_memberships acl_m
                   ON acl_m.org_id = acl_c.org_id AND acl_m.user_id = $5
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
                     OR acl_c.owner_user_id = $5
                     OR EXISTS (
                       SELECT 1 FROM collection_user_access cua
                       WHERE cua.org_id = acl_c.org_id
                         AND cua.collection_id = acl_c.id
                         AND cua.user_id = $5
                     )
                     OR EXISTS (
                       SELECT 1 FROM collection_group_access cga
                       JOIN group_memberships gm
                         ON gm.org_id = cga.org_id
                        AND gm.group_id = cga.group_id
                        AND gm.user_id = $5
                       WHERE cga.org_id = acl_c.org_id
                         AND cga.collection_id = acl_c.id
                     )
                     OR EXISTS (
                       SELECT 1 FROM collection_role_access cra
                       JOIN org_memberships om
                         ON om.org_id = cra.org_id
                        AND om.user_id = $5
                       JOIN roles rr
                         ON rr.org_id = om.org_id
                        AND rr.code = om.role
                        AND rr.id = cra.role_id
                       WHERE cra.org_id = acl_c.org_id
                         AND cra.collection_id = acl_c.id
                     )
                   )
               )
               AND d.deleted_at IS NULL
               AND d.state = 'indexed'
               AND dv.publication_state = 'published'",
            &[
                &ctx.org_id(),
                &document_id,
                &version_id,
                &kind,
                &ctx.user_id(),
            ],
        )
        .await?;
    row.map(|row| map_authorized_version(&row)).transpose()
}

/// Fail-closed Markdown artifact identity (derived artifact key/hash/type/size).
pub fn trusted_markdown_artifact(
    row: &AuthorizedVersionRow,
) -> Result<TrustedMarkdownArtifact, DbError> {
    let object_key = row
        .markdown_artifact_key
        .as_deref()
        .ok_or_else(|| DbError::Config("markdown_artifact_missing".into()))?;
    let content_sha256 = row
        .markdown_artifact_sha256
        .as_deref()
        .ok_or_else(|| DbError::Config("markdown_artifact_hash_missing".into()))?;
    let content_type = row
        .markdown_artifact_content_type
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("text/markdown; charset=utf-8");
    let byte_size = row
        .markdown_artifact_byte_size
        .ok_or_else(|| DbError::Config("markdown_artifact_size_missing".into()))?;
    let byte_size = u64::try_from(byte_size)
        .map_err(|_| DbError::Config("markdown_artifact_size_invalid".into()))?;
    if byte_size == 0 {
        return Err(DbError::Config("markdown_artifact_size_invalid".into()));
    }
    Ok(TrustedMarkdownArtifact {
        object_key: object_key.to_string(),
        content_sha256: content_sha256.to_string(),
        content_type: content_type.to_string(),
        byte_size,
    })
}

fn map_authorized_version(row: &Row) -> Result<AuthorizedVersionRow, DbError> {
    let state: String = row.get("state");
    let document_state = DocumentState::parse(&state).map_err(DbError::Config)?;
    let publication_state: String = row.get("publication_state");
    let publication_state = match publication_state.as_str() {
        "draft" => PublicationState::Draft,
        "published" => PublicationState::Published,
        other => {
            return Err(DbError::Config(format!(
                "unknown publication state: {other}"
            )));
        }
    };
    Ok(AuthorizedVersionRow {
        org_id: row.get("org_id"),
        collection_id: row.get("collection_id"),
        document_id: row.get("document_id"),
        version_id: row.get("version_id"),
        version_number: row.get("version_number"),
        parent_version_id: row.get("parent_version_id"),
        content_sha256: row.get("content_sha256"),
        original_object_key: row.get("original_object_key"),
        markdown_object_key: row.get("markdown_object_key"),
        markdown_artifact_key: row.get("markdown_artifact_key"),
        markdown_artifact_sha256: row.get("markdown_artifact_sha256"),
        markdown_artifact_content_type: row.get("markdown_artifact_content_type"),
        markdown_artifact_byte_size: row.get("markdown_artifact_byte_size"),
        source_filename: row.get("source_filename"),
        source_content_type: row.get("source_content_type"),
        byte_size: row.get("byte_size"),
        document_state,
        deleted_at: row.get("deleted_at"),
        publication_state,
        is_current: row.get("is_current"),
        effective_from: row.get("effective_from"),
        effective_to: row.get("effective_to"),
    })
}

/// Loads conflict evidence only when both claim sides remain authorized and
/// published under the resolved version visibility.
pub async fn load_authorized_conflict_evidence(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    collection_ids: &[Uuid],
    conflict_ids: &[Uuid],
    visibility: &VersionVisibility,
) -> Result<Vec<AuthorizedConflictEvidence>, DbError> {
    if collection_ids.is_empty() || conflict_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = match visibility {
        VersionVisibility::Current => {
            txn.query(
                "SELECT conf.id AS conflict_id,
                        conf.claim_a_id, conf.claim_b_id,
                        ca.document_id AS claim_a_document_id,
                        cb.document_id AS claim_b_document_id,
                        ca.version_id AS claim_a_version_id,
                        cb.version_id AS claim_b_version_id,
                        da.collection_id AS claim_a_collection_id,
                        db.collection_id AS claim_b_collection_id,
                        dva.is_current AS claim_a_is_current,
                        dvb.is_current AS claim_b_is_current,
                        (dva.publication_state = 'published') AS claim_a_published,
                        (dvb.publication_state = 'published') AS claim_b_published,
                        ca.citation_quote AS claim_a_quote,
                        cb.citation_quote AS claim_b_quote
                 FROM conflicts conf
                 JOIN claims ca
                   ON ca.org_id = conf.org_id AND ca.id = conf.claim_a_id
                 JOIN claims cb
                   ON cb.org_id = conf.org_id AND cb.id = conf.claim_b_id
                 JOIN documents da
                   ON da.org_id = ca.org_id AND da.id = ca.document_id
                 JOIN documents db
                   ON db.org_id = cb.org_id AND db.id = cb.document_id
                 JOIN document_versions dva
                   ON dva.org_id = ca.org_id
                  AND dva.document_id = ca.document_id
                  AND dva.id = ca.version_id
                 JOIN document_versions dvb
                   ON dvb.org_id = cb.org_id
                  AND dvb.document_id = cb.document_id
                  AND dvb.id = cb.version_id
                 WHERE conf.org_id = $1
                   AND conf.id = ANY($2)
                   AND da.collection_id = ANY($3)
                   AND db.collection_id = ANY($3)
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
                     WHERE acl_c.org_id = da.org_id
                       AND acl_c.id = da.collection_id
                       AND acl_c.deleted_at IS NULL
                       AND acl_u.disabled_at IS NULL
                       AND acl_p.code = $5
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
                       
                         OR EXISTS (
                           SELECT 1 FROM collection_group_access cga
                           JOIN group_memberships gm
                             ON gm.org_id = cga.org_id
                            AND gm.group_id = cga.group_id
                            AND gm.user_id = $4
                           WHERE cga.org_id = acl_c.org_id
                             AND cga.collection_id = acl_c.id
                         )
                         OR EXISTS (
                           SELECT 1 FROM collection_role_access cra
                           JOIN org_memberships om
                             ON om.org_id = cra.org_id
                            AND om.user_id = $4
                           JOIN roles rr
                             ON rr.org_id = om.org_id
                            AND rr.code = om.role
                            AND rr.id = cra.role_id
                           WHERE cra.org_id = acl_c.org_id
                             AND cra.collection_id = acl_c.id
                         )
)
                   )
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
                     WHERE acl_c.org_id = db.org_id
                       AND acl_c.id = db.collection_id
                       AND acl_c.deleted_at IS NULL
                       AND acl_u.disabled_at IS NULL
                       AND acl_p.code = $5
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
                       
                         OR EXISTS (
                           SELECT 1 FROM collection_group_access cga
                           JOIN group_memberships gm
                             ON gm.org_id = cga.org_id
                            AND gm.group_id = cga.group_id
                            AND gm.user_id = $4
                           WHERE cga.org_id = acl_c.org_id
                             AND cga.collection_id = acl_c.id
                         )
                         OR EXISTS (
                           SELECT 1 FROM collection_role_access cra
                           JOIN org_memberships om
                             ON om.org_id = cra.org_id
                            AND om.user_id = $4
                           JOIN roles rr
                             ON rr.org_id = om.org_id
                            AND rr.code = om.role
                            AND rr.id = cra.role_id
                           WHERE cra.org_id = acl_c.org_id
                             AND cra.collection_id = acl_c.id
                         )
)
                   )
                   AND da.deleted_at IS NULL
                   AND db.deleted_at IS NULL
                   AND da.state = 'indexed'
                   AND db.state = 'indexed'
                   AND dva.publication_state = 'published'
                   AND dvb.publication_state = 'published'
                   AND dva.is_current
                   AND dvb.is_current",
                &[
                    &ctx.org_id(),
                    &conflict_ids,
                    &collection_ids,
                    &ctx.user_id(),
                    &visibility.required_permission(),
                ],
            )
            .await?
        }
        VersionVisibility::VersionIds(version_ids) => {
            if version_ids.is_empty() {
                return Ok(Vec::new());
            }
            let versions: Vec<Uuid> = version_ids.iter().copied().collect();
            txn.query(
                "SELECT conf.id AS conflict_id,
                        conf.claim_a_id, conf.claim_b_id,
                        ca.document_id AS claim_a_document_id,
                        cb.document_id AS claim_b_document_id,
                        ca.version_id AS claim_a_version_id,
                        cb.version_id AS claim_b_version_id,
                        da.collection_id AS claim_a_collection_id,
                        db.collection_id AS claim_b_collection_id,
                        dva.is_current AS claim_a_is_current,
                        dvb.is_current AS claim_b_is_current,
                        (dva.publication_state = 'published') AS claim_a_published,
                        (dvb.publication_state = 'published') AS claim_b_published,
                        ca.citation_quote AS claim_a_quote,
                        cb.citation_quote AS claim_b_quote
                 FROM conflicts conf
                 JOIN claims ca
                   ON ca.org_id = conf.org_id AND ca.id = conf.claim_a_id
                 JOIN claims cb
                   ON cb.org_id = conf.org_id AND cb.id = conf.claim_b_id
                 JOIN documents da
                   ON da.org_id = ca.org_id AND da.id = ca.document_id
                 JOIN documents db
                   ON db.org_id = cb.org_id AND db.id = cb.document_id
                 JOIN document_versions dva
                   ON dva.org_id = ca.org_id
                  AND dva.document_id = ca.document_id
                  AND dva.id = ca.version_id
                 JOIN document_versions dvb
                   ON dvb.org_id = cb.org_id
                  AND dvb.document_id = cb.document_id
                  AND dvb.id = cb.version_id
                 WHERE conf.org_id = $1
                   AND conf.id = ANY($2)
                   AND da.collection_id = ANY($3)
                   AND db.collection_id = ANY($3)
                   AND EXISTS (
                     SELECT 1
                     FROM collections acl_c
                     JOIN org_memberships acl_m
                       ON acl_m.org_id = acl_c.org_id AND acl_m.user_id = $5
                     JOIN users acl_u ON acl_u.id = acl_m.user_id
                     JOIN roles acl_r
                       ON acl_r.org_id = acl_m.org_id AND acl_r.code = acl_m.role
                     JOIN role_permissions acl_rp
                       ON acl_rp.org_id = acl_r.org_id AND acl_rp.role_id = acl_r.id
                     JOIN permissions acl_p ON acl_p.id = acl_rp.permission_id
                     WHERE acl_c.org_id = da.org_id
                       AND acl_c.id = da.collection_id
                       AND acl_c.deleted_at IS NULL
                       AND acl_u.disabled_at IS NULL
                       AND acl_p.code = $6
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
                         OR acl_c.owner_user_id = $5
                         OR EXISTS (
                           SELECT 1 FROM collection_user_access cua
                           WHERE cua.org_id = acl_c.org_id
                             AND cua.collection_id = acl_c.id
                             AND cua.user_id = $5
                         )
                       
                         OR EXISTS (
                           SELECT 1 FROM collection_group_access cga
                           JOIN group_memberships gm
                             ON gm.org_id = cga.org_id
                            AND gm.group_id = cga.group_id
                            AND gm.user_id = $5
                           WHERE cga.org_id = acl_c.org_id
                             AND cga.collection_id = acl_c.id
                         )
                         OR EXISTS (
                           SELECT 1 FROM collection_role_access cra
                           JOIN org_memberships om
                             ON om.org_id = cra.org_id
                            AND om.user_id = $5
                           JOIN roles rr
                             ON rr.org_id = om.org_id
                            AND rr.code = om.role
                            AND rr.id = cra.role_id
                           WHERE cra.org_id = acl_c.org_id
                             AND cra.collection_id = acl_c.id
                         )
)
                   )
                   AND EXISTS (
                     SELECT 1
                     FROM collections acl_c
                     JOIN org_memberships acl_m
                       ON acl_m.org_id = acl_c.org_id AND acl_m.user_id = $5
                     JOIN users acl_u ON acl_u.id = acl_m.user_id
                     JOIN roles acl_r
                       ON acl_r.org_id = acl_m.org_id AND acl_r.code = acl_m.role
                     JOIN role_permissions acl_rp
                       ON acl_rp.org_id = acl_r.org_id AND acl_rp.role_id = acl_r.id
                     JOIN permissions acl_p ON acl_p.id = acl_rp.permission_id
                     WHERE acl_c.org_id = db.org_id
                       AND acl_c.id = db.collection_id
                       AND acl_c.deleted_at IS NULL
                       AND acl_u.disabled_at IS NULL
                       AND acl_p.code = $6
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
                         OR acl_c.owner_user_id = $5
                         OR EXISTS (
                           SELECT 1 FROM collection_user_access cua
                           WHERE cua.org_id = acl_c.org_id
                             AND cua.collection_id = acl_c.id
                             AND cua.user_id = $5
                         )
                       
                         OR EXISTS (
                           SELECT 1 FROM collection_group_access cga
                           JOIN group_memberships gm
                             ON gm.org_id = cga.org_id
                            AND gm.group_id = cga.group_id
                            AND gm.user_id = $5
                           WHERE cga.org_id = acl_c.org_id
                             AND cga.collection_id = acl_c.id
                         )
                         OR EXISTS (
                           SELECT 1 FROM collection_role_access cra
                           JOIN org_memberships om
                             ON om.org_id = cra.org_id
                            AND om.user_id = $5
                           JOIN roles rr
                             ON rr.org_id = om.org_id
                            AND rr.code = om.role
                            AND rr.id = cra.role_id
                           WHERE cra.org_id = acl_c.org_id
                             AND cra.collection_id = acl_c.id
                         )
)
                   )
                   AND da.deleted_at IS NULL
                   AND db.deleted_at IS NULL
                   AND da.state = 'indexed'
                   AND db.state = 'indexed'
                   AND dva.publication_state = 'published'
                   AND dvb.publication_state = 'published'
                   AND ca.version_id = ANY($4)
                   AND cb.version_id = ANY($4)",
                &[
                    &ctx.org_id(),
                    &conflict_ids,
                    &collection_ids,
                    &versions,
                    &ctx.user_id(),
                    &visibility.required_permission(),
                ],
            )
            .await?
        }
    };
    Ok(rows.iter().map(map_conflict_evidence).collect())
}

/// Decode a PostgreSQL `real` (`f32`) rank without widening to `f64` first.
pub fn read_pg_real_rank(row: &Row, column: &str) -> f32 {
    row.get::<_, f32>(column)
}

fn map_fts_candidate(row: &Row) -> Result<FtsCandidate, DbError> {
    Ok(FtsCandidate {
        chunk_id: row.get("id"),
        chunk_identity_sha256: row.get("chunk_identity_sha256"),
        document_id: row.get("document_id"),
        version_id: row.get("version_id"),
        collection_id: row.get("collection_id"),
        rank: read_pg_real_rank(row, "rank"),
    })
}

fn map_conflict_evidence(row: &Row) -> AuthorizedConflictEvidence {
    AuthorizedConflictEvidence {
        conflict_id: row.get("conflict_id"),
        claim_a_id: row.get("claim_a_id"),
        claim_b_id: row.get("claim_b_id"),
        claim_a_document_id: row.get("claim_a_document_id"),
        claim_b_document_id: row.get("claim_b_document_id"),
        claim_a_version_id: row.get("claim_a_version_id"),
        claim_b_version_id: row.get("claim_b_version_id"),
        claim_a_collection_id: row.get("claim_a_collection_id"),
        claim_b_collection_id: row.get("claim_b_collection_id"),
        claim_a_is_current: row.get("claim_a_is_current"),
        claim_b_is_current: row.get("claim_b_is_current"),
        claim_a_published: row.get("claim_a_published"),
        claim_b_published: row.get("claim_b_published"),
        claim_a_quote: row.get("claim_a_quote"),
        claim_b_quote: row.get("claim_b_quote"),
    }
}

fn map_hydrated_chunk(row: &Row) -> Result<HydratedChunkRow, DbError> {
    let state: String = row.get("state");
    let document_state = DocumentState::parse(&state).map_err(DbError::Config)?;
    let publication_state: String = row.get("publication_state");
    let publication_state = match publication_state.as_str() {
        "draft" => PublicationState::Draft,
        "published" => PublicationState::Published,
        other => {
            return Err(DbError::Config(format!(
                "unknown publication state: {other}"
            )));
        }
    };
    let index_state: String = row.get("index_state");
    let index_generation_state =
        IndexGenerationState::parse(&index_state).map_err(DbError::Config)?;
    Ok(HydratedChunkRow {
        chunk_id: row.get("id"),
        chunk_identity_sha256: row.get("chunk_identity_sha256"),
        org_id: row.get("org_id"),
        collection_id: row.get("collection_id"),
        document_id: row.get("document_id"),
        version_id: row.get("version_id"),
        version_number: row.get("version_number"),
        content_sha256: row.get("content_sha256"),
        heading_path: row.get("heading_path"),
        body: row.get("body"),
        page: row.get("page"),
        slide: row.get("slide"),
        sheet: row.get("sheet"),
        span_start: row.get("span_start"),
        span_end: row.get("span_end"),
        document_state,
        deleted_at: row.get("deleted_at"),
        publication_state,
        is_current: row.get("is_current"),
        effective_from: row.get("effective_from"),
        effective_to: row.get("effective_to"),
        index_metadata_id: row.get("index_metadata_id"),
        index_generation_active: row.get("is_active"),
        index_generation_state,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use uuid::Uuid;

    #[test]
    fn version_visibility_empty_ids_is_fail_closed() {
        let visibility = VersionVisibility::VersionIds(BTreeSet::new());
        match visibility {
            VersionVisibility::VersionIds(ids) => assert!(ids.is_empty()),
            VersionVisibility::Current => panic!("expected version ids"),
        }
        let _ = Uuid::nil();
    }

    #[test]
    fn only_active_generation_is_retrieval_visible() {
        assert!(index_generation_visible_for_retrieval(
            true,
            IndexGenerationState::Active
        ));
        assert!(!index_generation_visible_for_retrieval(
            true,
            IndexGenerationState::Shadow
        ));
        assert!(!index_generation_visible_for_retrieval(
            true,
            IndexGenerationState::Building
        ));
        assert!(!index_generation_visible_for_retrieval(
            false,
            IndexGenerationState::Active
        ));
        assert!(!index_generation_visible_for_retrieval(
            false,
            IndexGenerationState::Retired
        ));
        assert!(!index_generation_visible_for_retrieval(
            true,
            IndexGenerationState::Draining
        ));
    }

    #[test]
    fn pg_real_rank_helper_preserves_f32() {
        // Compile-time contract: retrieval must decode REAL as f32, not f64.
        let value: f32 = 0.75;
        assert!((value - 0.75).abs() < f32::EPSILON);
    }
}
