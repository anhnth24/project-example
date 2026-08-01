//! Publish a document version with dual-layer authz and co-committed audit.

use deadpool_postgres::Pool;
use thiserror::Error;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::auth::permissions::{
    require_operation_collection_access_on_txn, require_permission, ResolveError,
};
use crate::db::document_versions;
use crate::db::documents;
use crate::db::error::DbError;
use crate::db::models::{AccessLevel, AuditOutcome};
use crate::db::pool::with_org_txn_typed;
use crate::services::audit::{self, AuditAction, AuditRecord};

const PERMISSION_DOC_PUBLISH: &str = "doc.publish";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PublishError {
    #[error("permission denied")]
    PermissionDenied,
    #[error("not found")]
    NotFound,
    #[error("database error")]
    Database,
}

impl From<DbError> for PublishError {
    fn from(error: DbError) -> Self {
        match error {
            DbError::NotFound => Self::NotFound,
            _ => Self::Database,
        }
    }
}

fn map_resolve(error: ResolveError) -> PublishError {
    match error {
        ResolveError::PermissionDenied | ResolveError::CollectionDenied => {
            PublishError::PermissionDenied
        }
        _ => PublishError::Database,
    }
}

/// Publishes `version_id` for `document_id` and records `document.publish` in
/// the same transaction. Rolls back the publish if audit persistence fails.
pub async fn publish_version(
    pool: &Pool,
    ctx: &OrgContext,
    request_id: &str,
    document_id: Uuid,
    version_id: Uuid,
) -> Result<(), PublishError> {
    require_permission(ctx, PERMISSION_DOC_PUBLISH).map_err(map_resolve)?;
    with_org_txn_typed(pool, ctx, {
        let ctx = ctx.clone();
        let request_id = request_id.to_string();
        move |txn| {
            Box::pin(async move {
                let document = documents::get_by_id(txn, &ctx, document_id).await?;
                require_operation_collection_access_on_txn(
                    txn,
                    &ctx,
                    document.collection_id,
                    PERMISSION_DOC_PUBLISH,
                    AccessLevel::Write,
                )
                .await
                .map_err(map_resolve)?;
                document_versions::publish_version(txn, &ctx, document_id, version_id).await?;
                let resource_id = document_id.to_string();
                audit::record_in_txn(
                    txn,
                    &ctx,
                    AuditRecord {
                        request_id: &request_id,
                        action: AuditAction::DocumentPublish.as_str(),
                        resource_type: "document",
                        resource_id: Some(&resource_id),
                        outcome: AuditOutcome::Success,
                        metadata: serde_json::json!({
                            "document_id": document_id.to_string(),
                            "version_id": version_id.to_string(),
                        }),
                    },
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
}
