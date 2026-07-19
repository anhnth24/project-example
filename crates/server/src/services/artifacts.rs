//! Derived artifact staging for conversion promotion.

use bytes::Bytes;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::services::conversion::ConversionIdentity;
use crate::storage::keys::trusted_key;
use crate::storage::minio::{MinioClient, ObjectIdentityMeta};
use crate::storage::{ObjectKey, StorageError};

#[derive(Debug, Clone)]
pub struct MarkdownStageInput {
    pub collection_id: Option<Uuid>,
    pub document_id: Uuid,
    pub promoted_version_id: Uuid,
    pub staging_key: ObjectKey,
    pub markdown: Vec<u8>,
    pub markdown_sha256: String,
    pub markdown_len: u64,
}

#[derive(Debug, Clone)]
pub struct StagedMarkdown {
    pub key: ObjectKey,
    pub object_key: String,
    pub content_sha256: String,
    pub byte_size: u64,
    pub created_or_verified: bool,
}

#[derive(Debug, Error)]
pub enum ArtifactStageError {
    #[error("staged markdown length is invalid")]
    InvalidLength,
    #[error("staged markdown hash is invalid")]
    InvalidHash,
    #[error("storage error")]
    Storage(#[from] StorageError),
}

pub async fn stage_markdown(
    storage: &MinioClient,
    ctx: &OrgContext,
    input: MarkdownStageInput,
) -> Result<StagedMarkdown, ArtifactStageError> {
    let expected_len =
        usize::try_from(input.markdown_len).map_err(|_| ArtifactStageError::InvalidLength)?;
    if input.markdown.len() != expected_len {
        return Err(ArtifactStageError::InvalidLength);
    }
    let actual_sha256 = hex::encode(Sha256::digest(&input.markdown));
    if actual_sha256 != input.markdown_sha256 {
        return Err(ArtifactStageError::InvalidHash);
    }

    let key = input.staging_key;
    if !key.belongs_to_org(ctx.org_id()) || !key.belongs_to_version(input.promoted_version_id) {
        return Err(ArtifactStageError::Storage(StorageError::KeyOrgMismatch));
    }
    if storage.object_exists(ctx.org_id(), &key).await? {
        let metadata = storage.head_metadata(ctx.org_id(), &key).await?;
        if metadata.get("content-sha256").map(String::as_str)
            == Some(input.markdown_sha256.as_str())
            && metadata
                .get("disposition")
                .map(String::as_str)
                .is_some_and(|value| value == "staged" || value == "trusted")
        {
            return Ok(StagedMarkdown {
                object_key: key.as_str(),
                key,
                content_sha256: input.markdown_sha256,
                byte_size: input.markdown_len,
                created_or_verified: false,
            });
        }
        return Err(ArtifactStageError::Storage(StorageError::OwnershipConflict));
    }

    let meta = ObjectIdentityMeta {
        org_id: ctx.org_id(),
        collection_id: input.collection_id,
        document_id: Some(input.document_id),
        version_id: Some(input.promoted_version_id),
        original_filename: None,
        canonical_format: Some("md".into()),
        content_sha256: Some(input.markdown_sha256.clone()),
        content_length: Some(input.markdown_len),
        // The trusted object is not visible to readers until derived_artifacts is
        // inserted in the promote transaction.
        disposition: Some("staged".into()),
    };
    storage
        .put_object(
            ctx.org_id(),
            &key,
            Bytes::from(input.markdown),
            &meta,
            "text/markdown; charset=utf-8",
        )
        .await?;
    Ok(StagedMarkdown {
        object_key: key.as_str(),
        key,
        content_sha256: input.markdown_sha256,
        byte_size: input.markdown_len,
        created_or_verified: true,
    })
}

pub fn markdown_key(
    identity: &ConversionIdentity,
    promoted_version_id: Uuid,
    job_id: Uuid,
    attempts: i32,
) -> Result<ObjectKey, StorageError> {
    trusted_key(
        identity.org_id,
        promoted_version_id,
        identity.staged_markdown_object_id(job_id, attempts),
        None,
    )
}
