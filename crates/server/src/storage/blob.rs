//! Bounded object fetch API for preview / citation / download (P1B-R02).
//!
//! Callers supply authoritative size/hash/type expectations from PostgreSQL before
//! allocating. Implementations HEAD first and stream with an incremental hash.
//! Content-type is canonicalized (type/subtype + params lowercased) before compare.
//! Length short/truncation is a precondition/integrity failure; only exceeding the
//! authorized cap yields [`StorageError::ObjectTooLarge`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::storage::error::StorageError;
use crate::storage::keys::{authorize_key_for_org, ObjectKey};
use crate::storage::minio::MinioClient;

/// HEAD/metadata observed before a bounded GET.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectHead {
    pub content_length: u64,
    pub content_sha256: Option<String>,
    pub content_type: Option<String>,
}

/// Bytes returned by a bounded GET after size/hash checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedObject {
    pub bytes: Bytes,
    pub content_sha256: String,
    pub content_length: u64,
    pub content_type: Option<String>,
}

/// Authoritative expectation from PostgreSQL (fail closed on mismatch).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectExpectation<'a> {
    pub content_sha256: &'a str,
    pub content_length: u64,
    pub content_type: Option<&'a str>,
}

/// Canonical MIME comparison form: `type/subtype` lowercased; parameters sorted
/// by name, names/values lowercased, quotes stripped.
pub fn canonicalize_content_type(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 255 {
        return None;
    }
    let mut parts = trimmed.split(';');
    let base = parts.next()?.trim();
    if base.is_empty() || !base.contains('/') || base.contains(' ') {
        return None;
    }
    let mut out = base.to_ascii_lowercase();
    let mut params = Vec::new();
    for param in parts {
        let param = param.trim();
        if param.is_empty() {
            continue;
        }
        let (name, value) = match param.split_once('=') {
            Some((name, value)) => (name.trim(), value.trim().trim_matches('"')),
            None => (param, ""),
        };
        if name.is_empty() {
            return None;
        }
        params.push((name.to_ascii_lowercase(), value.to_ascii_lowercase()));
    }
    params.sort_by(|left, right| left.0.cmp(&right.0));
    for (name, value) in params {
        out.push_str("; ");
        out.push_str(&name);
        if !value.is_empty() {
            out.push('=');
            out.push_str(&value);
        }
    }
    Some(out)
}

pub fn content_types_equivalent(left: &str, right: &str) -> bool {
    match (
        canonicalize_content_type(left),
        canonicalize_content_type(right),
    ) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// Validates HEAD length/hash against an authorized expectation and byte cap.
/// Content-type is enforced after the body is observed (response or stored meta).
pub fn validate_head_against_expectation(
    head: &ObjectHead,
    max_bytes: u64,
    expected: &ObjectExpectation<'_>,
) -> Result<(), StorageError> {
    if expected.content_length == 0 || expected.content_length > max_bytes {
        return Err(StorageError::ObjectTooLarge);
    }
    if head.content_length > max_bytes {
        return Err(StorageError::ObjectTooLarge);
    }
    if head.content_length != expected.content_length {
        return Err(StorageError::PreconditionFailed);
    }
    if let Some(stored_hash) = head.content_sha256.as_deref() {
        if stored_hash != expected.content_sha256 {
            return Err(StorageError::PreconditionFailed);
        }
    }
    Ok(())
}

/// Incremental bounded reader used by Memory + MinIO paths (unit-tested without MinIO).
#[derive(Debug)]
pub struct BoundedAccumulator {
    max_bytes: u64,
    expected_len: u64,
    hasher: Sha256,
    total: u64,
    out: Vec<u8>,
}

impl BoundedAccumulator {
    pub fn begin(max_bytes: u64, expected_len: u64) -> Result<Self, StorageError> {
        if expected_len == 0 || expected_len > max_bytes {
            return Err(StorageError::ObjectTooLarge);
        }
        Ok(Self {
            max_bytes,
            expected_len,
            hasher: Sha256::new(),
            total: 0,
            out: Vec::with_capacity(expected_len.min(max_bytes) as usize),
        })
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<(), StorageError> {
        let next = self.total.saturating_add(chunk.len() as u64);
        if next > self.max_bytes {
            return Err(StorageError::ObjectTooLarge);
        }
        if next > self.expected_len {
            return Err(StorageError::PreconditionFailed);
        }
        self.total = next;
        self.hasher.update(chunk);
        self.out.extend_from_slice(chunk);
        Ok(())
    }

    pub fn finish(
        self,
        expected: &ObjectExpectation<'_>,
        observed_content_type: Option<String>,
    ) -> Result<FetchedObject, StorageError> {
        if self.total != self.expected_len || self.total != expected.content_length {
            return Err(StorageError::PreconditionFailed);
        }
        let content_sha256 = hex::encode(self.hasher.finalize());
        if content_sha256 != expected.content_sha256 {
            return Err(StorageError::PreconditionFailed);
        }
        if let Some(expected_type) = expected.content_type {
            let Some(actual) = observed_content_type.as_deref() else {
                return Err(StorageError::PreconditionFailed);
            };
            if !content_types_equivalent(expected_type, actual) {
                return Err(StorageError::PreconditionFailed);
            }
        }
        Ok(FetchedObject {
            bytes: Bytes::from(self.out),
            content_sha256,
            content_length: self.total,
            content_type: observed_content_type,
        })
    }
}

/// Org-bound blob reads used by citation/preview/download.
pub trait BlobStore: Send + Sync {
    fn head_object(
        &self,
        org_id: Uuid,
        key: &ObjectKey,
    ) -> impl std::future::Future<Output = Result<ObjectHead, StorageError>> + Send;

    fn get_object_bounded(
        &self,
        org_id: Uuid,
        key: &ObjectKey,
        max_bytes: u64,
        expected: &ObjectExpectation<'_>,
    ) -> impl std::future::Future<Output = Result<FetchedObject, StorageError>> + Send;
}

impl BlobStore for MinioClient {
    async fn head_object(&self, org_id: Uuid, key: &ObjectKey) -> Result<ObjectHead, StorageError> {
        self.head_object_meta(org_id, key).await
    }

    async fn get_object_bounded(
        &self,
        org_id: Uuid,
        key: &ObjectKey,
        max_bytes: u64,
        expected: &ObjectExpectation<'_>,
    ) -> Result<FetchedObject, StorageError> {
        MinioClient::get_object_bounded(self, org_id, key, max_bytes, expected).await
    }
}

/// In-memory blob store for hermetic citation/preview/download tests.
#[derive(Clone, Default)]
pub struct MemoryBlobStore {
    inner: Arc<Mutex<HashMap<String, MemoryObject>>>,
}

#[derive(Clone)]
struct MemoryObject {
    org_id: Uuid,
    bytes: Bytes,
    content_sha256: String,
    content_type: Option<String>,
}

impl MemoryBlobStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn put(
        &self,
        org_id: Uuid,
        key: &ObjectKey,
        bytes: impl Into<Bytes>,
        content_type: Option<&str>,
    ) -> Result<(), StorageError> {
        authorize_key_for_org(key, org_id)?;
        let bytes = bytes.into();
        let content_sha256 = hex::encode(Sha256::digest(&bytes));
        self.inner.lock().expect("memory blob store lock").insert(
            key.as_str(),
            MemoryObject {
                org_id,
                bytes,
                content_sha256,
                content_type: content_type.map(str::to_string),
            },
        );
        Ok(())
    }
}

impl BlobStore for MemoryBlobStore {
    async fn head_object(&self, org_id: Uuid, key: &ObjectKey) -> Result<ObjectHead, StorageError> {
        authorize_key_for_org(key, org_id)?;
        let guard = self.inner.lock().expect("memory blob store lock");
        let object = guard.get(&key.as_str()).ok_or(StorageError::NotFound)?;
        if object.org_id != org_id {
            return Err(StorageError::OwnershipConflict);
        }
        Ok(ObjectHead {
            content_length: object.bytes.len() as u64,
            content_sha256: Some(object.content_sha256.clone()),
            content_type: object.content_type.clone(),
        })
    }

    async fn get_object_bounded(
        &self,
        org_id: Uuid,
        key: &ObjectKey,
        max_bytes: u64,
        expected: &ObjectExpectation<'_>,
    ) -> Result<FetchedObject, StorageError> {
        let head = self.head_object(org_id, key).await?;
        validate_head_against_expectation(&head, max_bytes, expected)?;
        let guard = self.inner.lock().expect("memory blob store lock");
        let object = guard.get(&key.as_str()).ok_or(StorageError::NotFound)?;
        if object.org_id != org_id {
            return Err(StorageError::OwnershipConflict);
        }
        let mut acc = BoundedAccumulator::begin(max_bytes, expected.content_length)?;
        acc.push(&object.bytes)?;
        acc.finish(expected, object.content_type.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::keys::{quarantine_key, trusted_key};

    #[test]
    fn canonicalize_content_type_normalizes_case_and_param_order() {
        assert_eq!(
            canonicalize_content_type("Text/Markdown; Charset=\"UTF-8\"; x=1").as_deref(),
            Some("text/markdown; charset=utf-8; x=1")
        );
        assert!(content_types_equivalent(
            "text/markdown; charset=utf-8",
            "Text/Markdown; Charset=UTF-8"
        ));
        assert!(!content_types_equivalent(
            "text/markdown",
            "application/pdf"
        ));
    }

    #[test]
    fn bounded_accumulator_short_read_is_precondition_not_too_large() {
        let sha = hex::encode(Sha256::digest(b"abcd"));
        let expected = ObjectExpectation {
            content_sha256: &sha,
            content_length: 4,
            content_type: Some("text/plain"),
        };
        let mut acc = BoundedAccumulator::begin(64, 4).unwrap();
        acc.push(b"ab").unwrap();
        assert_eq!(
            acc.finish(&expected, Some("text/plain".into())),
            Err(StorageError::PreconditionFailed)
        );
    }

    #[test]
    fn bounded_accumulator_over_cap_is_object_too_large() {
        let mut acc = BoundedAccumulator::begin(4, 4).unwrap();
        assert_eq!(acc.push(b"abcde"), Err(StorageError::ObjectTooLarge));
        assert!(matches!(
            BoundedAccumulator::begin(4, 8),
            Err(StorageError::ObjectTooLarge)
        ));
    }

    #[test]
    fn bounded_accumulator_type_mismatch_is_precondition() {
        let body = Bytes::from_static(b"hello");
        let sha = hex::encode(Sha256::digest(&body));
        let expected = ObjectExpectation {
            content_sha256: &sha,
            content_length: body.len() as u64,
            content_type: Some("application/pdf"),
        };
        let mut acc = BoundedAccumulator::begin(64, expected.content_length).unwrap();
        acc.push(&body).unwrap();
        assert_eq!(
            acc.finish(&expected, Some("text/plain".into())),
            Err(StorageError::PreconditionFailed)
        );
    }

    #[tokio::test]
    async fn memory_store_enforces_type_short_and_oversize() {
        let store = MemoryBlobStore::new();
        let org = Uuid::new_v4();
        let other = Uuid::new_v4();
        let key = quarantine_key(org, Uuid::new_v4(), None).unwrap();
        let body = Bytes::from_static(b"hello-bytes");
        store
            .put(org, &key, body.clone(), Some("text/plain"))
            .unwrap();
        let sha = hex::encode(Sha256::digest(&body));
        let fetched = store
            .get_object_bounded(
                org,
                &key,
                64,
                &ObjectExpectation {
                    content_sha256: &sha,
                    content_length: body.len() as u64,
                    content_type: Some("Text/Plain"),
                },
            )
            .await
            .unwrap();
        assert_eq!(fetched.bytes, body);

        assert!(matches!(
            store
                .get_object_bounded(
                    org,
                    &key,
                    4,
                    &ObjectExpectation {
                        content_sha256: &sha,
                        content_length: body.len() as u64,
                        content_type: None,
                    },
                )
                .await,
            Err(StorageError::ObjectTooLarge)
        ));
        assert!(matches!(
            store
                .get_object_bounded(
                    org,
                    &key,
                    64,
                    &ObjectExpectation {
                        content_sha256: &sha,
                        content_length: body.len() as u64,
                        content_type: Some("application/pdf"),
                    },
                )
                .await,
            Err(StorageError::PreconditionFailed)
        ));
        assert!(matches!(
            store
                .get_object_bounded(
                    org,
                    &key,
                    64,
                    &ObjectExpectation {
                        content_sha256: &sha,
                        content_length: (body.len() as u64) + 1,
                        content_type: Some("text/plain"),
                    },
                )
                .await,
            Err(StorageError::PreconditionFailed)
        ));
        assert!(matches!(
            store.head_object(other, &key).await,
            Err(StorageError::KeyOrgMismatch)
        ));
        let trusted = trusted_key(org, Uuid::new_v4(), Uuid::new_v4(), None).unwrap();
        assert!(matches!(
            store.head_object(org, &trusted).await,
            Err(StorageError::NotFound)
        ));
    }
}
