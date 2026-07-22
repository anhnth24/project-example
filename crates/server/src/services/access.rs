//! Shared authorization resolver for documents/versions (P1B-R02 review).
//!
//! Rules:
//! - fail closed on missing/cross-scope documents (IDOR → not found)
//! - only published versions are readable
//! - non-current versions require `qa.history`
//! - suppressed/tombstoned documents are invisible

use deadpool_postgres::Pool;
use thiserror::Error;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;
use crate::db::models::{Document, DocumentVersion, PublicationState};
use crate::db::pool::with_org_txn_typed;
use crate::db::{document_versions, documents};
use crate::services::deletion::document_reads_suppressed;
use crate::services::retrieval::PERMISSION_QA_HISTORY;

#[derive(Debug, Clone)]
pub struct AuthorizedVersion {
    pub document: Document,
    pub version: DocumentVersion,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AccessError {
    #[error("not found")]
    NotFound,
    #[error("history permission required")]
    HistoryRequired,
    #[error("version is not published")]
    NotPublished,
    #[error("database error")]
    Database,
}

impl AccessError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::HistoryRequired => "history_permission_required",
            Self::NotPublished => "version_not_published",
            Self::Database => "database_error",
        }
    }
}

impl From<DbError> for AccessError {
    fn from(error: DbError) -> Self {
        match error {
            DbError::NotFound => Self::NotFound,
            _ => Self::Database,
        }
    }
}

/// Loads a document visible to the caller. Cross-scope / deleted → [`AccessError::NotFound`].
pub async fn resolve_document(
    pool: &Pool,
    ctx: &OrgContext,
    document_id: Uuid,
) -> Result<Document, AccessError> {
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let document = documents::get_by_id(txn, &ctx, document_id).await?;
                if document_reads_suppressed(document.state, document.deleted_at.is_some()) {
                    return Err(AccessError::NotFound);
                }
                if !ctx.allows_collection(document.collection_id) {
                    return Err(AccessError::NotFound);
                }
                Ok(document)
            })
        }
    })
    .await
}

/// Resolves a published version for preview/download/citation.
///
/// `version_id = None` selects the current published pointer.
pub async fn resolve_published_version(
    pool: &Pool,
    ctx: &OrgContext,
    document_id: Uuid,
    version_id: Option<Uuid>,
) -> Result<AuthorizedVersion, AccessError> {
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let document = documents::get_by_id(txn, &ctx, document_id).await?;
                if document_reads_suppressed(document.state, document.deleted_at.is_some()) {
                    return Err(AccessError::NotFound);
                }
                if !ctx.allows_collection(document.collection_id) {
                    return Err(AccessError::NotFound);
                }
                let version_id = match version_id.or(document.current_version_id) {
                    Some(id) => id,
                    None => return Err(AccessError::NotFound),
                };
                let version = document_versions::find_by_id(txn, &ctx, document_id, version_id)
                    .await?
                    .ok_or(AccessError::NotFound)?;
                if version.publication_state != PublicationState::Published {
                    return Err(AccessError::NotPublished);
                }
                if !version.is_current && !ctx.has_permission(PERMISSION_QA_HISTORY) {
                    return Err(AccessError::HistoryRequired);
                }
                Ok(AuthorizedVersion { document, version })
            })
        }
    })
    .await
}

/// Permission required to observe/enqueue jobs that lack a document scope.
pub const PERMISSION_JOBS_SYSTEM: &str = "jobs.system";

/// Hermetic dual-leg gate used by REST + retrieval conflict hydration.
pub fn conflict_both_legs_authorized(
    ctx: &OrgContext,
    collection_a: Uuid,
    collection_b: Uuid,
    published_a: bool,
    published_b: bool,
    deleted_or_tombstoned_a: bool,
    deleted_or_tombstoned_b: bool,
) -> bool {
    if deleted_or_tombstoned_a || deleted_or_tombstoned_b {
        return false;
    }
    if !(published_a && published_b) {
        return false;
    }
    ctx.allows_collection(collection_a) && ctx.allows_collection(collection_b)
}

/// Dual-leg JOIN + WHERE shared by conflict list/get/triage (both claim sides).
const CONFLICT_DUAL_LEG_SQL: &str = "
    FROM conflicts conf
    JOIN claims ca ON ca.org_id = conf.org_id AND ca.id = conf.claim_a_id
    JOIN claims cb ON cb.org_id = conf.org_id AND cb.id = conf.claim_b_id
    JOIN documents da ON da.org_id = ca.org_id AND da.id = ca.document_id
    JOIN documents db ON db.org_id = cb.org_id AND db.id = cb.document_id
    JOIN document_versions va
      ON va.org_id = ca.org_id
     AND va.document_id = ca.document_id
     AND va.id = ca.version_id
    JOIN document_versions vb
      ON vb.org_id = cb.org_id
     AND vb.document_id = cb.document_id
     AND vb.id = cb.version_id
    WHERE conf.org_id = $1
      AND da.collection_id = ANY($2::uuid[])
      AND db.collection_id = ANY($2::uuid[])
      AND da.deleted_at IS NULL
      AND db.deleted_at IS NULL
      AND da.state NOT IN ('tombstoned', 'purged')
      AND db.state NOT IN ('tombstoned', 'purged')
      AND va.publication_state = 'published'
      AND vb.publication_state = 'published'
";

/// Lists open conflicts where both evidence legs remain authorized.
pub async fn list_authorized_conflicts(
    pool: &Pool,
    ctx: &OrgContext,
) -> Result<Vec<tokio_postgres::Row>, AccessError> {
    let allowed: Vec<Uuid> = ctx.allowed_collection_ids().iter().copied().collect();
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let sql = format!(
                    "SELECT conf.id, conf.status, conf.severity, conf.conflict_type,
                            conf.claim_a_id, conf.claim_b_id,
                            conf.first_detected_at, conf.resolved_at,
                            da.collection_id AS collection_a_id,
                            db.collection_id AS collection_b_id
                     {CONFLICT_DUAL_LEG_SQL}
                       AND conf.status = 'open'
                     ORDER BY conf.first_detected_at DESC
                     LIMIT 100"
                );
                txn.query(sql.as_str(), &[&ctx.org_id(), &allowed])
                    .await
                    .map_err(DbError::from)
                    .map_err(AccessError::from)
            })
        }
    })
    .await
}

/// Loads one conflict when both evidence legs remain authorized.
pub async fn resolve_conflict(
    pool: &Pool,
    ctx: &OrgContext,
    conflict_id: Uuid,
) -> Result<tokio_postgres::Row, AccessError> {
    let allowed: Vec<Uuid> = ctx.allowed_collection_ids().iter().copied().collect();
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let sql = format!(
                    "SELECT conf.id, conf.status, conf.severity, conf.conflict_type,
                            conf.claim_a_id, conf.claim_b_id,
                            conf.resolution_note, conf.resolved_at,
                            da.collection_id AS collection_a_id,
                            db.collection_id AS collection_b_id
                     {CONFLICT_DUAL_LEG_SQL}
                       AND conf.id = $3"
                );
                txn.query_opt(sql.as_str(), &[&ctx.org_id(), &allowed, &conflict_id])
                    .await
                    .map_err(DbError::from)?
                    .ok_or(AccessError::NotFound)
            })
        }
    })
    .await
}

/// Triages an open conflict after dual-leg authorization.
pub async fn triage_authorized_conflict(
    pool: &Pool,
    ctx: &OrgContext,
    conflict_id: Uuid,
    status: &str,
    resolution_note: Option<&str>,
) -> Result<tokio_postgres::Row, AccessError> {
    let allowed: Vec<Uuid> = ctx.allowed_collection_ids().iter().copied().collect();
    let status = status.to_string();
    let note = resolution_note.map(str::to_string);
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                txn.query_opt(
                    "UPDATE conflicts conf
                     SET status = $3,
                         resolved_at = now(),
                         resolution_note = $4,
                         updated_at = now()
                     FROM claims ca, claims cb, documents da, documents db,
                          document_versions va, document_versions vb
                     WHERE conf.org_id = $1
                       AND conf.id = $2
                       AND conf.status = 'open'
                       AND ca.org_id = conf.org_id AND ca.id = conf.claim_a_id
                       AND cb.org_id = conf.org_id AND cb.id = conf.claim_b_id
                       AND da.org_id = ca.org_id AND da.id = ca.document_id
                       AND db.org_id = cb.org_id AND db.id = cb.document_id
                       AND va.org_id = ca.org_id AND va.document_id = ca.document_id
                           AND va.id = ca.version_id
                       AND vb.org_id = cb.org_id AND vb.document_id = cb.document_id
                           AND vb.id = cb.version_id
                       AND da.collection_id = ANY($5::uuid[])
                       AND db.collection_id = ANY($5::uuid[])
                       AND da.deleted_at IS NULL AND db.deleted_at IS NULL
                       AND da.state NOT IN ('tombstoned', 'purged')
                       AND db.state NOT IN ('tombstoned', 'purged')
                       AND va.publication_state = 'published'
                       AND vb.publication_state = 'published'
                     RETURNING conf.id, conf.status, conf.resolved_at",
                    &[&ctx.org_id(), &conflict_id, &status, &note, &allowed],
                )
                .await
                .map_err(DbError::from)?
                .ok_or(AccessError::NotFound)
            })
        }
    })
    .await
}

/// Authorizes a job for status/SSE. Missing / cross-collection → not found.
///
/// Documentless jobs are denied unless the caller has [`PERMISSION_JOBS_SYSTEM`].
pub async fn resolve_job_access(
    pool: &Pool,
    ctx: &OrgContext,
    job_id: Uuid,
) -> Result<crate::db::models::Job, AccessError> {
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let job = crate::db::jobs::get_by_id(txn, &ctx, job_id).await?;
                match job.document_id {
                    Some(document_id) => {
                        let document = documents::get_by_id(txn, &ctx, document_id).await?;
                        if document_reads_suppressed(document.state, document.deleted_at.is_some())
                            || !ctx.allows_collection(document.collection_id)
                        {
                            return Err(AccessError::NotFound);
                        }
                    }
                    None => {
                        if !ctx.has_permission(PERMISSION_JOBS_SYSTEM) {
                            return Err(AccessError::NotFound);
                        }
                    }
                }
                Ok(job)
            })
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::DocumentState;

    #[test]
    fn access_error_codes_are_stable() {
        assert_eq!(AccessError::NotFound.code(), "not_found");
        assert_eq!(
            AccessError::HistoryRequired.code(),
            "history_permission_required"
        );
    }

    #[test]
    fn suppressed_document_helper_agrees_with_access_policy() {
        assert!(document_reads_suppressed(DocumentState::Tombstoned, false));
        assert!(document_reads_suppressed(DocumentState::Indexed, true));
        assert!(!document_reads_suppressed(DocumentState::Indexed, false));
    }

    #[test]
    fn dual_leg_conflict_rejects_tombstone_unpublished_or_one_collection() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let ctx =
            OrgContext::try_new(Uuid::new_v4(), Uuid::new_v4(), ["qa.query"], [a, b]).unwrap();
        assert!(conflict_both_legs_authorized(
            &ctx, a, b, true, true, false, false
        ));
        assert!(!conflict_both_legs_authorized(
            &ctx, a, b, true, false, false, false
        ));
        assert!(!conflict_both_legs_authorized(
            &ctx, a, b, true, true, true, false
        ));
        assert!(!conflict_both_legs_authorized(
            &ctx, a, b, true, true, false, true
        ));
        let one = OrgContext::try_new(Uuid::new_v4(), Uuid::new_v4(), ["qa.query"], [a]).unwrap();
        assert!(!conflict_both_legs_authorized(
            &one, a, b, true, true, false, false
        ));
    }
}
