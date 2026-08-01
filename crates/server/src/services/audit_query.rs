//! Audit log read service (1C-11) with dual-layer `audit.view` enforcement.

use chrono::{DateTime, Utc};
use deadpool_postgres::Pool;
use thiserror::Error;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::auth::permissions::require_permission;
use crate::db::audit::{self as db_audit, AuditListFilter};
use crate::db::error::DbError;
use crate::db::models::AuditLogEntry;
use crate::db::pool::with_org_txn_typed;

const PERMISSION_AUDIT_VIEW: &str = "audit.view";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AuditQueryError {
    #[error("permission denied")]
    PermissionDenied,
    #[error("database error")]
    Database,
}

impl From<DbError> for AuditQueryError {
    fn from(_: DbError) -> Self {
        Self::Database
    }
}

/// Lists audit rows after requiring [`PERMISSION_AUDIT_VIEW`].
///
/// `db::audit::list_page` remains the storage primitive; callers that need
/// authorization must go through this service.
pub async fn list_page(
    pool: &Pool,
    ctx: &OrgContext,
    filter: &AuditListFilter,
    limit: i64,
    after_created_at: Option<DateTime<Utc>>,
    after_id: Option<Uuid>,
) -> Result<Vec<AuditLogEntry>, AuditQueryError> {
    require_permission(ctx, PERMISSION_AUDIT_VIEW)
        .map_err(|_| AuditQueryError::PermissionDenied)?;
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        let filter = filter.clone();
        move |txn| {
            Box::pin(async move {
                Ok(
                    db_audit::list_page(txn, &ctx, &filter, limit, after_created_at, after_id)
                        .await?,
                )
            })
        }
    })
    .await
}
