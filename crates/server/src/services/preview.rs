//! Authorized Markdown preview from trusted object storage (P1B-R02).
//!
//! Preview always re-checks org/collection ACL, published-only visibility, and
//! `qa.history` for non-current versions before fetching Markdown.

use deadpool_postgres::Pool;
use thiserror::Error;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::models::{Document, DocumentVersion};
use crate::services::access::{self, AccessError};
use crate::storage::keys::parse_key_for_org;
use crate::storage::minio::MinioClient;
use crate::storage::StorageError;

/// Hard bound for preview body returned to clients (UTF-8 Markdown).
pub const MAX_PREVIEW_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownPreview {
    pub document: Document,
    pub version: DocumentVersion,
    pub markdown: String,
    pub truncated: bool,
    /// Original source object content hash (`document_versions.content_sha256`).
    pub source_content_sha256: String,
    /// SHA-256 of the returned trusted Markdown bytes (canonical artifact).
    pub canonical_markdown_sha256: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PreviewError {
    #[error("permission denied")]
    PermissionDenied,
    #[error("history permission required")]
    HistoryRequired,
    #[error("document or version not found")]
    NotFound,
    #[error("version is not published")]
    NotPublished,
    #[error("document deleted or suspended")]
    Suppressed,
    #[error("markdown artifact unavailable")]
    ArtifactUnavailable,
    #[error("preview exceeds size bound")]
    TooLarge,
    #[error("database error")]
    Database,
    #[error("storage error")]
    Storage,
}

impl PreviewError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::PermissionDenied => "preview_permission_denied",
            Self::HistoryRequired => "preview_history_required",
            Self::NotFound => "preview_not_found",
            Self::NotPublished => "preview_not_published",
            Self::Suppressed => "preview_suppressed",
            Self::ArtifactUnavailable => "preview_artifact_unavailable",
            Self::TooLarge => "preview_too_large",
            Self::Database => "preview_database",
            Self::Storage => "preview_storage",
        }
    }
}

fn map_access(error: AccessError) -> PreviewError {
    match error {
        AccessError::NotFound => PreviewError::NotFound,
        AccessError::HistoryRequired => PreviewError::HistoryRequired,
        AccessError::NotPublished => PreviewError::NotPublished,
        AccessError::Database => PreviewError::Database,
    }
}

/// Loads trusted Markdown for a published document version after fresh authorization.
pub async fn preview_markdown(
    pool: &Pool,
    ctx: &OrgContext,
    store: &MinioClient,
    document_id: Uuid,
    version_id: Option<Uuid>,
) -> Result<MarkdownPreview, PreviewError> {
    let authorized = access::resolve_published_version(pool, ctx, document_id, version_id)
        .await
        .map_err(map_access)?;
    let document = authorized.document;
    let version = authorized.version;

    let markdown_key = version
        .markdown_object_key
        .as_deref()
        .ok_or(PreviewError::ArtifactUnavailable)?;
    let key = parse_key_for_org(markdown_key, ctx.org_id())
        .map_err(|_| PreviewError::ArtifactUnavailable)?;
    let bytes = store
        .get_object(ctx.org_id(), &key)
        .await
        .map_err(|error| match error {
            StorageError::NotFound => PreviewError::ArtifactUnavailable,
            StorageError::KeyOrgMismatch | StorageError::MissingScope => {
                PreviewError::PermissionDenied
            }
            _ => PreviewError::Storage,
        })?;
    if bytes.len() > MAX_PREVIEW_BYTES * 2 {
        return Err(PreviewError::TooLarge);
    }
    // Hash the full trusted artifact *before* any client truncation.
    let canonical_markdown_sha256 = {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(&bytes))
    };
    let truncated = bytes.len() > MAX_PREVIEW_BYTES;
    let slice = if truncated {
        &bytes[..MAX_PREVIEW_BYTES]
    } else {
        bytes.as_ref()
    };
    let markdown = String::from_utf8_lossy(slice).into_owned();
    Ok(MarkdownPreview {
        source_content_sha256: version.content_sha256.clone(),
        canonical_markdown_sha256,
        document,
        version,
        markdown,
        truncated,
    })
}

/// Pure helper: full-artifact digest is independent of the truncated response body.
pub fn preview_artifact_digest(full_bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(full_bytes))
}

#[cfg(test)]
mod tests {
    use super::{preview_artifact_digest, MAX_PREVIEW_BYTES};

    #[test]
    fn preview_bound_is_two_mebibytes() {
        assert_eq!(MAX_PREVIEW_BYTES, 2 * 1024 * 1024);
    }

    #[test]
    fn preview_hash_covers_full_artifact_before_truncate() {
        let mut full = vec![b'a'; MAX_PREVIEW_BYTES + 64];
        full.extend_from_slice(b"TAIL");
        let digest = preview_artifact_digest(&full);
        let truncated = &full[..MAX_PREVIEW_BYTES];
        assert_ne!(digest, preview_artifact_digest(truncated));
        assert_eq!(digest.len(), 64);
    }

    #[test]
    fn source_hash_differs_from_markdown_hash() {
        let source = b"%PDF-source-bytes";
        let markdown = b"# Canonical Markdown\n";
        assert_ne!(
            preview_artifact_digest(source),
            preview_artifact_digest(markdown)
        );
    }
}
