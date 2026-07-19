//! Trusted Markdown preview retrieval with fresh document authorization.

use bytes::Bytes;
use deadpool_postgres::Pool;
use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::document_versions;
use crate::db::error::DbError;
use crate::db::pool::with_org_txn_typed;
use crate::storage::keys::{authorize_key_for_version, parse_key_for_org};
use crate::storage::minio::MinioClient;
use crate::storage::{ObjectNamespace, StorageError};

pub const MARKDOWN_CONTENT_TYPE: &str = "text/markdown; charset=utf-8";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkdownPreview {
    #[serde(skip_serializing)]
    pub bytes: Bytes,
    pub content_type: &'static str,
    pub content_sha256: String,
    pub version_id: Uuid,
    pub version_number: i32,
    pub byte_size: u64,
}

#[derive(Debug, Error)]
pub enum PreviewError {
    #[error("preview was not found")]
    NotFound,
    #[error("database error")]
    Db(#[from] DbError),
    #[error("storage error")]
    Storage(#[from] StorageError),
    #[error("preview integrity check failed")]
    Integrity,
}

#[derive(Debug)]
struct AuthorizedPreview {
    version_number: i32,
}

pub async fn fetch_markdown_preview(
    pool: &Pool,
    storage: &MinioClient,
    ctx: &OrgContext,
    document_id: Uuid,
    version_id: Uuid,
) -> Result<MarkdownPreview, PreviewError> {
    if ctx.org_id().is_nil() || ctx.allowed_collection_ids().is_empty() {
        return Err(PreviewError::NotFound);
    }
    let authorized = ctx
        .allowed_collection_ids()
        .iter()
        .copied()
        .collect::<Vec<_>>();
    let txn_ctx = ctx.clone();
    let query_ctx = txn_ctx.clone();
    let (source, artifact) = with_org_txn_typed(pool, &txn_ctx, move |txn| {
        Box::pin(async move {
            let row = txn
                .query_opt(
                    "SELECT v.version_number
                     FROM document_versions v
                     JOIN documents d
                       ON d.org_id = v.org_id
                      AND d.id = v.document_id
                     WHERE v.org_id = $1
                       AND v.document_id = $3
                       AND v.id = $4
                       AND d.collection_id = ANY($2::uuid[])
                       AND d.state = 'indexed'
                       AND d.deleted_at IS NULL",
                    &[&query_ctx.org_id(), &authorized, &document_id, &version_id],
                )
                .await
                .map_err(DbError::from)?
                .ok_or(PreviewError::NotFound)?;
            let artifact = document_versions::find_markdown_artifact(txn, &query_ctx, version_id)
                .await?
                .ok_or(PreviewError::NotFound)?;
            Ok::<_, PreviewError>((
                AuthorizedPreview {
                    version_number: row.get("version_number"),
                },
                artifact,
            ))
        })
    })
    .await?;

    let key = parse_key_for_org(&artifact.object_key, ctx.org_id())?;
    if key.namespace() != ObjectNamespace::Trusted {
        return Err(PreviewError::NotFound);
    }
    authorize_key_for_version(&key, version_id).map_err(|_| PreviewError::NotFound)?;
    let bytes = storage.get_object(ctx.org_id(), &key).await?;
    let actual_sha256 = hex::encode(Sha256::digest(&bytes));
    if actual_sha256 != artifact.content_sha256 {
        return Err(PreviewError::Integrity);
    }
    Ok(MarkdownPreview {
        byte_size: bytes.len() as u64,
        bytes,
        content_type: MARKDOWN_CONTENT_TYPE,
        content_sha256: artifact.content_sha256,
        version_id,
        version_number: source.version_number,
    })
}
