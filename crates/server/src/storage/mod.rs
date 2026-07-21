//! Fail-closed storage adapters (MinIO object store + Qdrant vectors).
//!
//! Tenant-facing exports deliberately omit [`qdrant::QdrantAdminClient`] —
//! operator collection drops require a distinct admin API key and an explicit
//! import of `crate::storage::qdrant::{QdrantAdminClient, QdrantAdminApiKey}`.

pub mod blob;
pub mod error;
pub mod keys;
pub mod minio;
pub mod qdrant;
pub mod url_safety;

pub use blob::{
    canonicalize_content_type, content_types_equivalent, validate_head_against_expectation,
    BlobStore, BoundedAccumulator, FetchedObject, MemoryBlobStore, ObjectExpectation, ObjectHead,
};
pub use error::StorageError;
pub use keys::{
    authorize_key_for_org, authorize_key_for_version, parse_key_for_org, quarantine_key,
    trusted_key, ObjectKey, ObjectNamespace,
};
pub use minio::{MinioClient, ObjectIdentityMeta};
pub use qdrant::{
    point_id_from_org_collection_and_chunk, ChunkPointPayload, QdrantClient, SearchHit,
    UpsertPoint, VectorScope,
};
