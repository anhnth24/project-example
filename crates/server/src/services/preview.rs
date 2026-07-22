//! Trusted Markdown preview fetch with fresh authorization (P1B-R02).
//!
//! Object keys and bucket names are never accepted from clients. The server loads
//! derived-artifact key/hash/type/size from PostgreSQL, HEADs, then bounded-reads.

use deadpool_postgres::Pool;
use thiserror::Error;
use uuid::Uuid;

use crate::auth::permissions::{resolve_org_context_in_txn, ResolveError};
use crate::db::error::DbError;
use crate::db::pool::with_org_txn;
use crate::db::search::{self, AuthorizedVersionRow, TrustedMarkdownArtifact};
use crate::services::deletion::document_reads_suppressed;
use crate::services::retrieval::{PERMISSION_QA_HISTORY, PERMISSION_QA_QUERY};
use crate::storage::blob::{BlobStore, ObjectExpectation};
use crate::storage::keys::{authorize_key_for_version, parse_key_for_org, ObjectNamespace};
use crate::storage::StorageError;

/// Small preview cap — full originals use download capabilities instead.
pub const PREVIEW_MAX_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedMarkdownPreview {
    pub org_id: Uuid,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub version_number: i32,
    pub content_sha256: String,
    pub markdown_sha256: String,
    pub is_current: bool,
    pub markdown: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PreviewError {
    #[error("permission denied")]
    PermissionDenied,
    #[error("preview not found")]
    NotFound,
    #[error("markdown artifact is missing")]
    MarkdownMissing,
    #[error("markdown integrity check failed")]
    Integrity,
    #[error("markdown is not utf-8")]
    InvalidUtf8,
    #[error("markdown exceeds preview size bound")]
    TooLarge,
    #[error("storage unavailable")]
    StorageUnavailable,
    #[error("storage error")]
    Storage,
    #[error("database error")]
    Database,
}

impl PreviewError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::PermissionDenied => "preview_permission_denied",
            Self::NotFound => "preview_not_found",
            Self::MarkdownMissing => "preview_markdown_missing",
            Self::Integrity => "preview_integrity",
            Self::InvalidUtf8 => "preview_invalid_utf8",
            Self::TooLarge => "preview_too_large",
            Self::StorageUnavailable => "preview_storage_unavailable",
            Self::Storage => "preview_storage",
            Self::Database => "preview_database",
        }
    }
}

impl From<ResolveError> for PreviewError {
    fn from(_: ResolveError) -> Self {
        Self::PermissionDenied
    }
}

impl From<DbError> for PreviewError {
    fn from(value: DbError) -> Self {
        match value {
            DbError::Config(ref msg) if msg.starts_with("markdown_artifact_") => {
                Self::MarkdownMissing
            }
            _ => Self::Database,
        }
    }
}

impl From<StorageError> for PreviewError {
    fn from(value: StorageError) -> Self {
        match value {
            StorageError::NotFound => Self::NotFound,
            StorageError::KeyOrgMismatch
            | StorageError::MissingScope
            | StorageError::InvalidKey => Self::PermissionDenied,
            StorageError::ObjectTooLarge => Self::TooLarge,
            StorageError::PreconditionFailed => Self::Integrity,
            _ => Self::Storage,
        }
    }
}

/// Fail-closed derived Markdown artifact (no key/hash fallback).
pub fn trusted_markdown_artifact_from_version(
    row: &AuthorizedVersionRow,
) -> Result<TrustedMarkdownArtifact, PreviewError> {
    if document_reads_suppressed(row.document_state, row.deleted_at.is_some()) {
        return Err(PreviewError::PermissionDenied);
    }
    search::trusted_markdown_artifact(row).map_err(PreviewError::from)
}

/// Fetches trusted Markdown for preview. Client-supplied keys/buckets are ignored.
pub async fn fetch_trusted_markdown<S: BlobStore>(
    pool: &Pool,
    storage: &S,
    org_id: Uuid,
    user_id: Uuid,
    document_id: Uuid,
    version_id: Uuid,
) -> Result<TrustedMarkdownPreview, PreviewError> {
    let ctx = resolve_org_context_in_txn(pool, org_id, user_id).await?;
    if !ctx.has_permission(PERMISSION_QA_QUERY) {
        return Err(PreviewError::PermissionDenied);
    }
    if ctx.allowed_collection_ids().is_empty() {
        return Err(PreviewError::PermissionDenied);
    }

    let row = with_org_txn(pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                search::load_authorized_version_for_read(txn, &ctx, document_id, version_id).await
            })
        }
    })
    .await?
    .ok_or(PreviewError::NotFound)?;

    if row.org_id != org_id || !ctx.allows_collection(row.collection_id) {
        return Err(PreviewError::PermissionDenied);
    }
    if !row.is_current && !ctx.has_permission(PERMISSION_QA_HISTORY) {
        return Err(PreviewError::PermissionDenied);
    }

    let artifact = trusted_markdown_artifact_from_version(&row)?;
    if artifact.byte_size > PREVIEW_MAX_BYTES {
        return Err(PreviewError::TooLarge);
    }
    let key = parse_key_for_org(&artifact.object_key, org_id)?;
    if key.namespace() != ObjectNamespace::Trusted {
        return Err(PreviewError::PermissionDenied);
    }
    authorize_key_for_version(&key, version_id)?;

    let fetched = storage
        .get_object_bounded(
            org_id,
            &key,
            PREVIEW_MAX_BYTES,
            &ObjectExpectation {
                content_sha256: &artifact.content_sha256,
                content_length: artifact.byte_size,
                content_type: Some(artifact.content_type.as_str()),
            },
        )
        .await?;
    let markdown =
        String::from_utf8(fetched.bytes.to_vec()).map_err(|_| PreviewError::InvalidUtf8)?;

    Ok(TrustedMarkdownPreview {
        org_id,
        document_id: row.document_id,
        version_id: row.version_id,
        version_number: row.version_number,
        content_sha256: row.content_sha256,
        markdown_sha256: fetched.content_sha256,
        is_current: row.is_current,
        markdown,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::{DocumentState, PublicationState};
    use chrono::{TimeZone, Utc};

    fn sample_version() -> AuthorizedVersionRow {
        AuthorizedVersionRow {
            org_id: Uuid::new_v4(),
            collection_id: Uuid::new_v4(),
            document_id: Uuid::new_v4(),
            version_id: Uuid::new_v4(),
            version_number: 1,
            parent_version_id: Some(Uuid::new_v4()),
            content_sha256: "a".repeat(64),
            original_object_key: "quarantine/aa/bb".into(),
            markdown_object_key: Some("trusted/aa/bb/cc".into()),
            markdown_artifact_key: Some("trusted/aa/bb/cc".into()),
            markdown_artifact_sha256: Some("b".repeat(64)),
            markdown_artifact_content_type: Some("text/markdown; charset=utf-8".into()),
            markdown_artifact_byte_size: Some(128),
            source_filename: Some("policy.pdf".into()),
            source_content_type: Some("text/markdown; charset=utf-8".into()),
            byte_size: Some(128),
            document_state: DocumentState::Indexed,
            deleted_at: None,
            publication_state: PublicationState::Published,
            is_current: true,
            effective_from: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
            effective_to: None,
        }
    }

    #[test]
    fn requires_derived_artifact_hash_and_size() {
        let row = sample_version();
        let artifact = trusted_markdown_artifact_from_version(&row).unwrap();
        assert_eq!(artifact.content_sha256, "b".repeat(64));
        assert_eq!(artifact.byte_size, 128);

        let mut missing_hash = sample_version();
        missing_hash.markdown_artifact_sha256 = None;
        assert_eq!(
            trusted_markdown_artifact_from_version(&missing_hash),
            Err(PreviewError::MarkdownMissing)
        );

        let mut fallback_only = sample_version();
        fallback_only.markdown_artifact_key = None;
        // markdown_object_key alone is not enough without immutable artifact hash/size.
        assert_eq!(
            trusted_markdown_artifact_from_version(&fallback_only),
            Err(PreviewError::MarkdownMissing)
        );
    }

    #[test]
    fn tombstoned_version_cannot_preview() {
        let mut row = sample_version();
        row.document_state = DocumentState::Tombstoned;
        assert_eq!(
            trusted_markdown_artifact_from_version(&row),
            Err(PreviewError::PermissionDenied)
        );
    }
}
