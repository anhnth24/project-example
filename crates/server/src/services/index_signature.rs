//! Index-signature → Qdrant collection naming (ADR 0006 / 0009).
//!
//! One shared Qdrant collection per index generation. The generation identity is
//! `fileconv_knowledge::identity::IndexSignature::digest()`; the collection name
//! embeds the **full** 64-hex digest so generations cannot collide.
//!
//! All Qdrant operations take a validated [`CollectionName`] — never a raw string.

use std::fmt;

use fileconv_knowledge::identity::IndexSignature;

use crate::storage::error::StorageError;

/// Prefix for versioned Markhand chunk collections.
pub const COLLECTION_NAME_PREFIX: &str = "markhand_chunks_";
/// Full SHA-256 hex digest length embedded in the collection name (256 bits).
pub const DIGEST_HEX_LEN: usize = 64;

/// Validated Qdrant collection name: `markhand_chunks_<64 lowercase hex>`.
///
/// Charset is restricted to `[a-z0-9_]` so the value is safe as a URL path
/// segment. Construct only via [`collection_name_for_digest`],
/// [`collection_name_for_signature`], or [`parse_collection_name`].
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct CollectionName(String);

impl CollectionName {
    /// Borrow the validated name string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Digest portion after the `markhand_chunks_` prefix.
    pub fn digest(&self) -> &str {
        &self.0[COLLECTION_NAME_PREFIX.len()..]
    }
}

impl fmt::Debug for CollectionName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Names are generation digests (not secrets); still avoid dumping full
        // strings in sparse logs by showing prefix only.
        formatter
            .debug_struct("CollectionName")
            .field("prefix", &COLLECTION_NAME_PREFIX)
            .field("digest_len", &DIGEST_HEX_LEN)
            .finish_non_exhaustive()
    }
}

impl AsRef<str> for CollectionName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// Build the Qdrant collection name for a full 64-char signature digest.
pub fn collection_name_for_digest(digest: &str) -> Result<CollectionName, StorageError> {
    validate_signature_digest(digest)?;
    let raw = format!("{COLLECTION_NAME_PREFIX}{digest}");
    parse_collection_name(&raw)
}

/// Build the Qdrant collection name for an [`IndexSignature`].
pub fn collection_name_for_signature(
    signature: &IndexSignature<'_>,
) -> Result<CollectionName, StorageError> {
    let digest = signature
        .try_digest()
        .map_err(|_| StorageError::PreconditionFailed)?;
    collection_name_for_digest(&digest)
}

/// Parse and validate `markhand_chunks_<64-hex>` (charset `[a-z0-9_]` only).
pub fn parse_collection_name(name: &str) -> Result<CollectionName, StorageError> {
    if name.is_empty()
        || name.len() != COLLECTION_NAME_PREFIX.len() + DIGEST_HEX_LEN
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(StorageError::PreconditionFailed);
    }
    let Some(digest) = name.strip_prefix(COLLECTION_NAME_PREFIX) else {
        return Err(StorageError::PreconditionFailed);
    };
    validate_signature_digest(digest)?;
    Ok(CollectionName(name.to_string()))
}

/// Validate a full 64-character lowercase hex index signature digest.
pub fn validate_signature_digest(digest: &str) -> Result<(), StorageError> {
    if digest.len() != DIGEST_HEX_LEN
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(StorageError::PreconditionFailed);
    }
    Ok(())
}

/// True when `collection_name` was derived from `digest`.
pub fn collection_matches_digest(
    collection_name: &CollectionName,
    digest: &str,
) -> Result<bool, StorageError> {
    let expected = collection_name_for_digest(digest)?;
    Ok(collection_name == &expected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fileconv_knowledge::identity::{
        BODY_TEXT_VERSION, DEFAULT_CHUNKING_VERSION, QUERY_NORMALIZATION_VERSION,
        RUNTIME_LOCAL_HASH,
    };

    fn sample_signature() -> IndexSignature<'static> {
        IndexSignature {
            runtime_path: RUNTIME_LOCAL_HASH,
            embedding_family: "test-family",
            embedding_revision: "r1",
            dimensions: 8,
            normalized: true,
            chunking_version: DEFAULT_CHUNKING_VERSION,
            body_text_version: BODY_TEXT_VERSION,
            query_normalization_version: QUERY_NORMALIZATION_VERSION,
        }
    }

    #[test]
    fn collection_name_uses_full_digest() {
        let signature = sample_signature();
        let digest = signature.digest();
        let name = collection_name_for_signature(&signature).unwrap();
        assert!(name.as_str().starts_with(COLLECTION_NAME_PREFIX));
        assert_eq!(name.digest(), digest.as_str());
        assert_eq!(parse_collection_name(name.as_str()).unwrap(), name);
        assert!(collection_matches_digest(&name, &digest).unwrap());
    }

    #[test]
    fn rejects_invalid_digest_and_name() {
        assert!(collection_name_for_digest("short").is_err());
        assert!(collection_name_for_digest(&"A".repeat(64)).is_err());
        assert!(parse_collection_name("other_chunks_abcdef0123456789").is_err());
        assert!(parse_collection_name(&format!("markhand_chunks_{}", "A".repeat(64))).is_err());
        assert!(parse_collection_name("markhand_chunks_../etc/passwd").is_err());
        assert!(parse_collection_name(&format!("markhand_chunks_{}?x=1", "a".repeat(64))).is_err());
    }

    #[test]
    fn collection_name_rejects_unvalidated_runtime_paths_before_hashing() {
        for runtime_path in ["", "local-hash\n", "local_hash_v1"] {
            let signature = IndexSignature {
                runtime_path,
                ..sample_signature()
            };
            assert_eq!(
                collection_name_for_signature(&signature),
                Err(StorageError::PreconditionFailed),
                "runtime_path {runtime_path:?}"
            );
        }
    }
}
