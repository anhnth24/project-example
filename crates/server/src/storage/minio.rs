//! MinIO (S3-compatible) adapter with opaque keys and identity metadata.
//!
//! Client choice: `rust-s3` (crate lib name `s3`) with `tokio-rustls-tls` and
//! path-style addressing. Lighter than `aws-sdk-s3`, works against MinIO, and
//! avoids anonymous/public bucket helpers (`Bucket::new_public` is never used).
//!
//! Every put/get/delete/exists is org-bound: the key's org-opaque segment must
//! match the authorized org, and stored object metadata org must match on read
//! mutation paths (fail closed).

use std::collections::HashMap;

use bytes::Bytes;
use s3::creds::Credentials;
use s3::error::S3Error;
use s3::region::Region;
use s3::serde_types::HeadObjectResult;
use s3::Bucket;
use s3::BucketConfiguration;
use uuid::Uuid;

use crate::config::MinioConfig;
use crate::storage::error::StorageError;
use crate::storage::keys::{
    authorize_key_for_org, authorize_key_for_version, parse_key_structure, ObjectKey,
    ObjectNamespace,
};

/// Identity fields stored as S3 object metadata (never in the key path).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectIdentityMeta {
    pub org_id: Uuid,
    pub collection_id: Option<Uuid>,
    pub document_id: Option<Uuid>,
    pub version_id: Option<Uuid>,
    /// Original filename for display only — never written into the object key.
    pub original_filename: Option<String>,
    /// Server-derived canonical format (upload intake).
    pub canonical_format: Option<String>,
    /// Hex SHA-256 of object bytes.
    pub content_sha256: Option<String>,
    /// Declared content length in bytes.
    pub content_length: Option<u64>,
    /// Intake disposition (`accepted` / `quarantined`).
    pub disposition: Option<String>,
}

/// Fail-closed MinIO/S3 object store client (credentials required).
#[derive(Clone)]
pub struct MinioClient {
    bucket: Box<Bucket>,
    bucket_name: String,
}

impl std::fmt::Debug for MinioClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MinioClient")
            .field("bucket_name", &self.bucket_name)
            .finish_non_exhaustive()
    }
}

impl MinioClient {
    /// Build a client from typed config. Rejects empty credentials (fail closed).
    pub fn from_config(config: &MinioConfig) -> Result<Self, StorageError> {
        if config.access_key().expose().is_empty() || config.secret_key().expose().is_empty() {
            return Err(StorageError::ConfigMissingCredentials);
        }
        if config.bucket().trim().is_empty() {
            return Err(StorageError::ConfigInvalid);
        }
        let credentials = Credentials::new(
            Some(config.access_key().expose()),
            Some(config.secret_key().expose()),
            None,
            None,
            None,
        )
        .map_err(|_| StorageError::ConfigInvalid)?;
        let region = Region::Custom {
            region: config.region().to_string(),
            endpoint: config.endpoint().to_string(),
        };
        let mut bucket = Bucket::new(config.bucket(), region, credentials)
            .map_err(|_| StorageError::ConfigInvalid)?;
        if config.path_style() {
            bucket = bucket.with_path_style();
        }
        Ok(Self {
            bucket,
            bucket_name: config.bucket().to_string(),
        })
    }

    pub fn bucket_name(&self) -> &str {
        &self.bucket_name
    }

    /// Create the configured bucket if missing (test / bootstrap helper).
    pub async fn ensure_bucket(&self) -> Result<(), StorageError> {
        let region = self.bucket.region.clone();
        let credentials = self
            .bucket
            .credentials()
            .await
            .map_err(|_| StorageError::Transport)?;
        let config = BucketConfiguration::default();
        match Bucket::create_with_path_style(&self.bucket_name, region, credentials, config).await {
            Ok(response)
                if response.success()
                    || response.response_code == 409
                    || response.response_code == 200
                    || response.response_code == 204 =>
            {
                Ok(())
            }
            Ok(_) => Err(StorageError::Backend),
            Err(error) => {
                if is_bucket_already_exists(&error) {
                    Ok(())
                } else {
                    Err(StorageError::Transport)
                }
            }
        }
    }

    /// Put bytes under an opaque key owned by `org_id`, with identity metadata.
    pub async fn put_object(
        &self,
        org_id: Uuid,
        key: &ObjectKey,
        body: Bytes,
        meta: &ObjectIdentityMeta,
        content_type: &str,
    ) -> Result<(), StorageError> {
        authorize_key_for_org(key, org_id)?;
        if meta.org_id != org_id || meta.org_id.is_nil() {
            return Err(StorageError::KeyOrgMismatch);
        }
        if key.namespace() == ObjectNamespace::Trusted {
            let version_id = meta.version_id.ok_or(StorageError::MissingScope)?;
            authorize_key_for_version(key, version_id)?;
        }
        // Structural re-parse (defense in depth).
        let _ = parse_key_structure(&key.as_str())?;

        let path = key.as_str();
        let mut builder = self
            .bucket
            .put_object_builder(&path, body.as_ref())
            .with_content_type(content_type);
        builder = builder
            .with_metadata("org-id", meta.org_id.to_string())
            .map_err(|_| StorageError::ConfigInvalid)?;
        if let Some(collection_id) = meta.collection_id {
            builder = builder
                .with_metadata("collection-id", collection_id.to_string())
                .map_err(|_| StorageError::ConfigInvalid)?;
        }
        if let Some(document_id) = meta.document_id {
            builder = builder
                .with_metadata("document-id", document_id.to_string())
                .map_err(|_| StorageError::ConfigInvalid)?;
        }
        if let Some(version_id) = meta.version_id {
            builder = builder
                .with_metadata("version-id", version_id.to_string())
                .map_err(|_| StorageError::ConfigInvalid)?;
        }
        if let Some(filename) = meta.original_filename.as_deref() {
            let safe: String = filename
                .chars()
                .filter(|ch| !ch.is_control())
                .take(255)
                .collect();
            if !safe.is_empty() {
                builder = builder
                    .with_metadata("original-filename", safe)
                    .map_err(|_| StorageError::ConfigInvalid)?;
            }
        }
        if let Some(format) = meta.canonical_format.as_deref() {
            builder = builder
                .with_metadata("canonical-format", format)
                .map_err(|_| StorageError::ConfigInvalid)?;
        }
        if let Some(sha) = meta.content_sha256.as_deref() {
            builder = builder
                .with_metadata("content-sha256", sha)
                .map_err(|_| StorageError::ConfigInvalid)?;
        }
        if let Some(len) = meta.content_length {
            builder = builder
                .with_metadata("content-length-bytes", len.to_string())
                .map_err(|_| StorageError::ConfigInvalid)?;
        }
        if let Some(disposition) = meta.disposition.as_deref() {
            builder = builder
                .with_metadata("disposition", disposition)
                .map_err(|_| StorageError::ConfigInvalid)?;
        }
        let response = builder
            .execute()
            .await
            .map_err(|_| StorageError::Transport)?;
        if (200..300).contains(&response.status_code()) {
            Ok(())
        } else {
            Err(StorageError::Backend)
        }
    }

    /// Stream an `AsyncRead` into an opaque key (bounded memory; S3 multipart under the hood).
    pub async fn put_object_stream<R>(
        &self,
        org_id: Uuid,
        key: &ObjectKey,
        mut reader: R,
        meta: &ObjectIdentityMeta,
        content_type: &str,
    ) -> Result<(), StorageError>
    where
        R: tokio::io::AsyncRead + Unpin + Send,
    {
        authorize_key_for_org(key, org_id)?;
        if meta.org_id != org_id || meta.org_id.is_nil() {
            return Err(StorageError::KeyOrgMismatch);
        }
        if key.namespace() == ObjectNamespace::Trusted {
            let version_id = meta.version_id.ok_or(StorageError::MissingScope)?;
            authorize_key_for_version(key, version_id)?;
        }
        let _ = parse_key_structure(&key.as_str())?;
        let path = key.as_str();

        let mut builder = self
            .bucket
            .put_object_stream_builder(&path)
            .with_content_type(content_type);
        builder = builder
            .with_metadata("org-id", meta.org_id.to_string())
            .map_err(|_| StorageError::ConfigInvalid)?;
        if let Some(collection_id) = meta.collection_id {
            builder = builder
                .with_metadata("collection-id", collection_id.to_string())
                .map_err(|_| StorageError::ConfigInvalid)?;
        }
        if let Some(document_id) = meta.document_id {
            builder = builder
                .with_metadata("document-id", document_id.to_string())
                .map_err(|_| StorageError::ConfigInvalid)?;
        }
        if let Some(version_id) = meta.version_id {
            builder = builder
                .with_metadata("version-id", version_id.to_string())
                .map_err(|_| StorageError::ConfigInvalid)?;
        }
        if let Some(filename) = meta.original_filename.as_deref() {
            let safe: String = filename
                .chars()
                .filter(|ch| !ch.is_control())
                .take(255)
                .collect();
            if !safe.is_empty() {
                builder = builder
                    .with_metadata("original-filename", safe)
                    .map_err(|_| StorageError::ConfigInvalid)?;
            }
        }
        if let Some(format) = meta.canonical_format.as_deref() {
            builder = builder
                .with_metadata("canonical-format", format)
                .map_err(|_| StorageError::ConfigInvalid)?;
        }
        if let Some(sha) = meta.content_sha256.as_deref() {
            builder = builder
                .with_metadata("content-sha256", sha)
                .map_err(|_| StorageError::ConfigInvalid)?;
        }
        if let Some(len) = meta.content_length {
            builder = builder
                .with_metadata("content-length-bytes", len.to_string())
                .map_err(|_| StorageError::ConfigInvalid)?;
        }
        if let Some(disposition) = meta.disposition.as_deref() {
            builder = builder
                .with_metadata("disposition", disposition)
                .map_err(|_| StorageError::ConfigInvalid)?;
        }

        let response = builder
            .execute_stream(&mut reader)
            .await
            .map_err(|_| StorageError::Transport)?;
        if (200..300).contains(&response.status_code()) {
            Ok(())
        } else {
            Err(StorageError::Backend)
        }
    }

    pub async fn get_object(&self, org_id: Uuid, key: &ObjectKey) -> Result<Bytes, StorageError> {
        authorize_key_for_org(key, org_id)?;
        self.verify_stored_org(org_id, key).await?;
        let path = key.as_str();
        let response = self
            .bucket
            .get_object(&path)
            .await
            .map_err(map_s3_get_error)?;
        let status = response.status_code();
        if status == 404 {
            return Err(StorageError::NotFound);
        }
        if !(200..300).contains(&status) {
            return Err(StorageError::Backend);
        }
        Ok(Bytes::copy_from_slice(response.as_slice()))
    }

    pub async fn object_exists(&self, org_id: Uuid, key: &ObjectKey) -> Result<bool, StorageError> {
        authorize_key_for_org(key, org_id)?;
        match self.head_for_org(org_id, key).await {
            Ok(_) => Ok(true),
            Err(StorageError::NotFound) => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub async fn delete_object(&self, org_id: Uuid, key: &ObjectKey) -> Result<(), StorageError> {
        authorize_key_for_org(key, org_id)?;
        self.verify_stored_org(org_id, key).await?;
        let path = key.as_str();
        let response = self
            .bucket
            .delete_object(&path)
            .await
            .map_err(map_s3_get_error)?;
        let status = response.status_code();
        if status == 404 {
            return Err(StorageError::NotFound);
        }
        if (200..300).contains(&status) {
            Ok(())
        } else {
            Err(StorageError::Backend)
        }
    }

    /// Read identity metadata via HEAD after org authorization.
    pub async fn head_metadata(
        &self,
        org_id: Uuid,
        key: &ObjectKey,
    ) -> Result<HashMap<String, String>, StorageError> {
        authorize_key_for_org(key, org_id)?;
        let (head, _) = self.head_for_org(org_id, key).await?;
        Ok(metadata_map(&head))
    }

    async fn verify_stored_org(&self, org_id: Uuid, key: &ObjectKey) -> Result<(), StorageError> {
        let _ = self.head_for_org(org_id, key).await?;
        Ok(())
    }

    async fn head_for_org(
        &self,
        org_id: Uuid,
        key: &ObjectKey,
    ) -> Result<(HeadObjectResult, u16), StorageError> {
        let path = key.as_str();
        let (head, status) = match self.bucket.head_object(&path).await {
            Ok(pair) => pair,
            Err(error) => return Err(map_s3_head_error(error)),
        };
        if status == 404 {
            return Err(StorageError::NotFound);
        }
        if !(200..300).contains(&status) {
            return Err(StorageError::Backend);
        }
        let meta = metadata_map(&head);
        let Some(stored_org) = meta.get("org-id") else {
            return Err(StorageError::OwnershipConflict);
        };
        let stored = Uuid::parse_str(stored_org).map_err(|_| StorageError::OwnershipConflict)?;
        if stored != org_id {
            return Err(StorageError::OwnershipConflict);
        }
        Ok((head, status))
    }
}

fn metadata_map(head: &HeadObjectResult) -> HashMap<String, String> {
    let mut meta = HashMap::new();
    if let Some(map) = &head.metadata {
        for (k, v) in map {
            meta.insert(k.to_ascii_lowercase(), v.clone());
        }
    }
    meta
}

fn map_s3_get_error(error: S3Error) -> StorageError {
    match error {
        S3Error::HttpFailWithBody(404, _) => StorageError::NotFound,
        S3Error::HttpFailWithBody(_, _) => StorageError::Backend,
        S3Error::HttpFail => StorageError::Backend,
        _ => StorageError::Transport,
    }
}

fn map_s3_head_error(error: S3Error) -> StorageError {
    map_s3_get_error(error)
}

fn is_bucket_already_exists(error: &S3Error) -> bool {
    matches!(error, S3Error::HttpFailWithBody(409, _))
}
