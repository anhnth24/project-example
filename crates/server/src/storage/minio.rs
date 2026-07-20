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
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::TryStreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, CONTENT_LENGTH, CONTENT_TYPE};
use s3::creds::Credentials;
use s3::error::S3Error;
use s3::region::Region;
use s3::serde_types::HeadObjectResult;
use s3::Bucket;
use s3::BucketConfiguration;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::time::timeout;
use uuid::Uuid;

use crate::config::MinioConfig;
use crate::services::upload::STREAM_CHUNK_BYTES;
use crate::storage::error::StorageError;
use crate::storage::keys::{
    authorize_key_for_org, authorize_key_for_version, parse_key_structure, ObjectKey,
    ObjectNamespace,
};

/// Observed identity fields from a stored object HEAD (reconcile validation).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ObservedObjectIdentity {
    pub org_id: Option<Uuid>,
    pub document_id: Option<Uuid>,
    pub version_id: Option<Uuid>,
    pub content_sha256: Option<String>,
    pub content_length: Option<u64>,
}

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

#[derive(Debug, Clone, Copy)]
pub struct ObjectPutVerification<'a> {
    pub expected_len: u64,
    pub expected_sha256: &'a str,
}

struct OwnedPutRequest {
    org_id: Uuid,
    key: ObjectKey,
    meta: ObjectIdentityMeta,
    content_type: String,
    expected_len: u64,
    expected_sha256: String,
}

/// Fail-closed MinIO/S3 object store client (credentials required).
#[derive(Clone)]
pub struct MinioClient {
    bucket: Box<Bucket>,
    bucket_name: String,
    http_client: reqwest::Client,
    operation_timeout: Duration,
}

impl std::fmt::Debug for MinioClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MinioClient")
            .field("bucket_name", &self.bucket_name)
            .field("operation_timeout", &self.operation_timeout)
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
        let operation_timeout = Duration::from_secs(config.operation_timeout_secs());
        let http_client = reqwest::Client::builder()
            .connect_timeout(operation_timeout)
            .timeout(operation_timeout)
            .build()
            .map_err(|_| StorageError::ConfigInvalid)?;
        Ok(Self {
            bucket,
            bucket_name: config.bucket().to_string(),
            http_client,
            operation_timeout,
        })
    }

    pub fn bucket_name(&self) -> &str {
        &self.bucket_name
    }

    /// Create the configured bucket if missing (test / bootstrap helper).
    pub async fn ensure_bucket(&self) -> Result<(), StorageError> {
        let region = self.bucket.region.clone();
        let credentials = self
            .with_s3_timeout(self.bucket.credentials(), |_| StorageError::Transport)
            .await?;
        let config = BucketConfiguration::default();
        match timeout(
            self.operation_timeout,
            Bucket::create_with_path_style(&self.bucket_name, region, credentials, config),
        )
        .await
        .map_err(|_| StorageError::Transport)?
        {
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
        let response = self
            .with_s3_timeout(builder.execute(), |_| StorageError::Transport)
            .await?;
        if (200..300).contains(&response.status_code()) {
            Ok(())
        } else {
            Err(StorageError::Backend)
        }
    }

    /// Stream an `AsyncRead` into an opaque key with fixed-memory single-PUT upload.
    pub async fn put_object_stream<R>(
        &self,
        org_id: Uuid,
        key: &ObjectKey,
        reader: R,
        meta: &ObjectIdentityMeta,
        content_type: &str,
        verification: ObjectPutVerification<'_>,
    ) -> Result<(), StorageError>
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        let client = self.clone();
        let request = OwnedPutRequest {
            org_id,
            key: key.clone(),
            meta: meta.clone(),
            content_type: content_type.to_string(),
            expected_len: verification.expected_len,
            expected_sha256: verification.expected_sha256.to_string(),
        };
        let handle =
            tokio::spawn(async move { client.put_object_stream_owned(reader, request).await });
        handle.await.map_err(|_| StorageError::Transport)?
    }

    async fn put_object_stream_owned<R>(
        &self,
        reader: R,
        request: OwnedPutRequest,
    ) -> Result<(), StorageError>
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        authorize_key_for_org(&request.key, request.org_id)?;
        if request.meta.org_id != request.org_id || request.meta.org_id.is_nil() {
            return Err(StorageError::KeyOrgMismatch);
        }
        if request.key.namespace() == ObjectNamespace::Trusted {
            let version_id = request.meta.version_id.ok_or(StorageError::MissingScope)?;
            authorize_key_for_version(&request.key, version_id)?;
        }
        let _ = parse_key_structure(&request.key.as_str())?;
        if request.meta.content_length != Some(request.expected_len)
            || request.meta.content_sha256.as_deref() != Some(request.expected_sha256.as_str())
        {
            return Err(StorageError::ConfigInvalid);
        }
        let generated_key = request.key.clone();
        // I07 reconciles checkpointed dead-letter staging keys; uncheckpointed crash
        // windows still need a future durable generated-object intent.
        let path = generated_key.as_str();
        let mut headers = identity_headers(&request.meta, &request.content_type)?;
        headers.insert(
            CONTENT_LENGTH,
            HeaderValue::from_str(&request.expected_len.to_string())
                .map_err(|_| StorageError::ConfigInvalid)?,
        );
        let presigned = self
            .with_s3_timeout(
                self.bucket
                    .presign_put(&path, 300, Some(headers.clone()), None),
                |_| StorageError::Transport,
            )
            .await?;
        let uploaded = Arc::new(AtomicU64::new(0));
        let uploaded_for_stream = Arc::clone(&uploaded);
        let body_stream = futures::stream::try_unfold(reader, move |mut reader| {
            let uploaded = Arc::clone(&uploaded_for_stream);
            async move {
                let mut buf = vec![0_u8; STREAM_CHUNK_BYTES];
                let n = reader
                    .read(&mut buf)
                    .await
                    .map_err(|_| std::io::Error::other("upload stream failed"))?;
                if n == 0 {
                    return Ok(None);
                }
                buf.truncate(n);
                uploaded.fetch_add(n as u64, Ordering::Relaxed);
                Ok::<_, std::io::Error>(Some((Bytes::from(buf), reader)))
            }
        });
        let response = match timeout(
            self.operation_timeout,
            self.http_client
                .put(presigned)
                .headers(headers)
                .body(reqwest::Body::wrap_stream(body_stream))
                .send(),
        )
        .await
        {
            Ok(Ok(response)) => response,
            Ok(Err(_)) | Err(_) => {
                return self
                    .cleanup_after_failed_put(
                        request.org_id,
                        &generated_key,
                        StorageError::Transport,
                    )
                    .await;
            }
        };
        if !response.status().is_success() {
            self.cleanup_after_failed_put(request.org_id, &generated_key, StorageError::Backend)
                .await?;
            return Err(StorageError::Backend);
        }
        if uploaded.load(Ordering::Relaxed) != request.expected_len {
            self.cleanup_after_failed_put(request.org_id, &generated_key, StorageError::Backend)
                .await?;
            return Err(StorageError::Backend);
        }
        if let Err(error) = self
            .verify_stored_object(
                request.org_id,
                &generated_key,
                request.expected_len,
                &request.expected_sha256,
            )
            .await
        {
            return self
                .cleanup_after_failed_put(request.org_id, &generated_key, error)
                .await;
        }
        Ok(())
    }

    pub async fn get_object(&self, org_id: Uuid, key: &ObjectKey) -> Result<Bytes, StorageError> {
        authorize_key_for_org(key, org_id)?;
        self.verify_stored_org(org_id, key).await?;
        let path = key.as_str();
        let response = self
            .with_s3_timeout(self.bucket.get_object(&path), map_s3_get_error)
            .await?;
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

    /// Observe stored identity metadata for reconcile validation (HEAD only).
    pub async fn observe_object_identity(
        &self,
        org_id: Uuid,
        key: &ObjectKey,
    ) -> Result<Option<ObservedObjectIdentity>, StorageError> {
        authorize_key_for_org(key, org_id)?;
        match self.head_for_org(org_id, key).await {
            Ok((head, _)) => {
                let meta = metadata_map(&head);
                let content_length = head
                    .content_length
                    .and_then(|len| u64::try_from(len).ok())
                    .or_else(|| {
                        meta.get("content-length-bytes")
                            .and_then(|value| value.parse().ok())
                    });
                Ok(Some(ObservedObjectIdentity {
                    org_id: meta
                        .get("org-id")
                        .and_then(|value| Uuid::parse_str(value).ok()),
                    document_id: meta
                        .get("document-id")
                        .and_then(|value| Uuid::parse_str(value).ok()),
                    version_id: meta
                        .get("version-id")
                        .and_then(|value| Uuid::parse_str(value).ok()),
                    content_sha256: meta.get("content-sha256").cloned(),
                    content_length,
                }))
            }
            Err(StorageError::NotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Lists object keys under an org-authorized prefix (bounded inventory scan).
    pub async fn list_keys_with_prefix(
        &self,
        org_id: Uuid,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<String>, StorageError> {
        if org_id.is_nil() || prefix.trim().is_empty() || limit == 0 {
            return Err(StorageError::MissingScope);
        }
        let org_opaque = crate::storage::keys::opaque_identity("org", org_id);
        let trusted = format!("trusted/{org_opaque}/");
        let quarantine = format!("quarantine/{org_opaque}/");
        if !(prefix.starts_with(&trusted) || prefix.starts_with(&quarantine)) {
            return Err(StorageError::OwnershipConflict);
        }
        let mut out = Vec::new();
        let mut continuation = None;
        loop {
            let (page, status) = self
                .with_s3_timeout(
                    self.bucket.list_page(
                        prefix.to_string(),
                        None,
                        continuation.clone(),
                        None,
                        Some(limit.saturating_sub(out.len()).max(1)),
                    ),
                    map_s3_get_error,
                )
                .await?;
            if !(200..300).contains(&status) {
                return Err(StorageError::Backend);
            }
            for object in page.contents {
                let key = object.key;
                if key.is_empty() {
                    continue;
                }
                let parsed = parse_key_structure(&key)?;
                authorize_key_for_org(&parsed, org_id)?;
                out.push(key);
                if out.len() >= limit {
                    return Ok(out);
                }
            }
            if !page.is_truncated {
                break;
            }
            continuation = page.next_continuation_token;
            if continuation.is_none() {
                break;
            }
        }
        Ok(out)
    }

    pub async fn delete_object(&self, org_id: Uuid, key: &ObjectKey) -> Result<(), StorageError> {
        authorize_key_for_org(key, org_id)?;
        self.verify_stored_org(org_id, key).await?;
        self.delete_object_authorized_key(key, false).await
    }

    /// Internal cleanup for objects whose key was just generated for `org_id`.
    ///
    /// This intentionally authorizes by the generated key instead of stored metadata so
    /// verification-failed objects with missing/mismatched metadata can still be removed.
    pub(crate) async fn cleanup_generated_object(
        &self,
        org_id: Uuid,
        key: &ObjectKey,
    ) -> Result<(), StorageError> {
        authorize_key_for_org(key, org_id)?;
        let _ = parse_key_structure(&key.as_str())?;
        self.delete_object_authorized_key(key, true).await
    }

    async fn delete_object_authorized_key(
        &self,
        key: &ObjectKey,
        missing_ok: bool,
    ) -> Result<(), StorageError> {
        let path = key.as_str();
        let response = self
            .with_s3_timeout(self.bucket.delete_object(&path), map_s3_get_error)
            .await?;
        let status = response.status_code();
        if status == 404 {
            return if missing_ok {
                Ok(())
            } else {
                Err(StorageError::NotFound)
            };
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

    async fn cleanup_after_failed_put(
        &self,
        org_id: Uuid,
        key: &ObjectKey,
        cause: StorageError,
    ) -> Result<(), StorageError> {
        match self.cleanup_generated_object(org_id, key).await {
            Ok(()) => Err(cause),
            Err(cleanup_error) => {
                eprintln!(
                    "fileconv-server: cleanup of failed upload object failed: {}",
                    cleanup_error.code()
                );
                Err(cause)
            }
        }
    }

    async fn verify_stored_object(
        &self,
        org_id: Uuid,
        key: &ObjectKey,
        expected_len: u64,
        expected_sha256: &str,
    ) -> Result<(), StorageError> {
        let (head, _) = self.head_for_org(org_id, key).await?;
        let stored_len = head.content_length.ok_or(StorageError::Backend)?;
        if stored_len < 0 || stored_len as u64 != expected_len {
            return Err(StorageError::Backend);
        }
        let meta = metadata_map(&head);
        if meta.get("content-sha256").map(String::as_str) != Some(expected_sha256) {
            return Err(StorageError::Backend);
        }
        let path = key.as_str();
        let url = self
            .with_s3_timeout(self.bucket.presign_get(path, 300, None), |_| {
                StorageError::Transport
            })
            .await?;
        let response = timeout(self.operation_timeout, self.http_client.get(url).send())
            .await
            .map_err(|_| StorageError::Transport)?
            .map_err(|_| StorageError::Transport)?;
        if !response.status().is_success() {
            return Err(StorageError::Backend);
        }
        let mut hasher = Sha256::new();
        let mut total = 0_u64;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream
            .try_next()
            .await
            .map_err(|_| StorageError::Transport)?
        {
            total = total.saturating_add(chunk.len() as u64);
            if total > expected_len {
                return Err(StorageError::Backend);
            }
            hasher.update(&chunk);
        }
        if total != expected_len || hex::encode(hasher.finalize()) != expected_sha256 {
            return Err(StorageError::Backend);
        }
        Ok(())
    }

    async fn head_for_org(
        &self,
        org_id: Uuid,
        key: &ObjectKey,
    ) -> Result<(HeadObjectResult, u16), StorageError> {
        let path = key.as_str();
        let (head, status) = match self
            .with_s3_timeout(self.bucket.head_object(&path), map_s3_head_error)
            .await
        {
            Ok(pair) => pair,
            Err(error) => return Err(error),
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

    async fn with_s3_timeout<T, F, M>(&self, future: F, map_error: M) -> Result<T, StorageError>
    where
        F: Future<Output = Result<T, S3Error>>,
        M: FnOnce(S3Error) -> StorageError,
    {
        timeout(self.operation_timeout, future)
            .await
            .map_err(|_| StorageError::Transport)?
            .map_err(map_error)
    }
}

fn identity_headers(
    meta: &ObjectIdentityMeta,
    content_type: &str,
) -> Result<HeaderMap, StorageError> {
    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_str(content_type).map_err(|_| StorageError::ConfigInvalid)?,
    );
    insert_header(&mut headers, "x-amz-meta-org-id", &meta.org_id.to_string())?;
    if let Some(collection_id) = meta.collection_id {
        insert_header(
            &mut headers,
            "x-amz-meta-collection-id",
            &collection_id.to_string(),
        )?;
    }
    if let Some(document_id) = meta.document_id {
        insert_header(
            &mut headers,
            "x-amz-meta-document-id",
            &document_id.to_string(),
        )?;
    }
    if let Some(version_id) = meta.version_id {
        insert_header(
            &mut headers,
            "x-amz-meta-version-id",
            &version_id.to_string(),
        )?;
    }
    if let Some(filename) = meta.original_filename.as_deref() {
        let safe: String = filename
            .chars()
            .filter(|ch| !ch.is_control())
            .take(255)
            .collect();
        if !safe.is_empty() {
            insert_header(&mut headers, "x-amz-meta-original-filename", &safe)?;
        }
    }
    if let Some(format) = meta.canonical_format.as_deref() {
        insert_header(&mut headers, "x-amz-meta-canonical-format", format)?;
    }
    if let Some(sha) = meta.content_sha256.as_deref() {
        insert_header(&mut headers, "x-amz-meta-content-sha256", sha)?;
    }
    if let Some(len) = meta.content_length {
        insert_header(
            &mut headers,
            "x-amz-meta-content-length-bytes",
            &len.to_string(),
        )?;
    }
    if let Some(disposition) = meta.disposition.as_deref() {
        insert_header(&mut headers, "x-amz-meta-disposition", disposition)?;
    }
    Ok(headers)
}

fn insert_header(
    headers: &mut HeaderMap,
    name: &'static str,
    value: &str,
) -> Result<(), StorageError> {
    headers.insert(
        HeaderName::from_static(name),
        HeaderValue::from_str(value).map_err(|_| StorageError::ConfigInvalid)?,
    );
    Ok(())
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
