//! Database models, pool, and tenant-scoped repositories.

pub mod chunks;
pub mod claims;
pub mod collections;
pub mod document_versions;
pub mod documents;
pub mod download_capabilities;
pub mod embedding_batches;
pub mod error;
pub mod index_metadata;
pub(crate) mod jobs;
pub mod models;
pub mod orgs;
pub mod pool;
pub mod quota;
pub mod search;
pub mod vector_cleanup_intents;
