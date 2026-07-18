use sha2::{Digest, Sha256};
use std::fmt::Write;

/// Identity digest schema version. Bump when field layout changes.
pub const IDENTITY_VERSION: u16 = 2;

pub const BODY_TEXT_VERSION: &str = "nfc-v1";
pub const QUERY_NORMALIZATION_VERSION: &str = "accent-fold-v1";
pub const DEFAULT_CHUNKING_VERSION: &str = "heading-chunks-2000-v1";

pub const RUNTIME_LOCAL_HASH: &str = "local-hash";
pub const RUNTIME_GLM_CLOUD_INTERIM: &str = "glm-cloud-interim";
pub const RUNTIME_VLLM_LOCAL: &str = "vllm-local";
pub const RUNTIME_PROVIDER_CLOUD: &str = "provider-cloud";

fn update_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

fn digest(domain: &str, fields: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"markhand-knowledge-identity");
    hasher.update(IDENTITY_VERSION.to_be_bytes());
    update_field(&mut hasher, domain.as_bytes());
    for field in fields {
        update_field(&mut hasher, field);
    }
    let bytes = hasher.finalize();
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

pub fn document_identity(source_rel: &str, content_sha256: &str) -> String {
    digest(
        "document",
        &[source_rel.as_bytes(), content_sha256.as_bytes()],
    )
}

/// Canonical chunk identity. Includes `version_id` so versions never share IDs.
pub fn chunk_identity(
    document_id: &str,
    version_id: &str,
    ordinal: u64,
    heading_path: &str,
    body: &str,
    body_text_version: &str,
) -> String {
    digest(
        "chunk",
        &[
            document_id.as_bytes(),
            version_id.as_bytes(),
            &ordinal.to_be_bytes(),
            heading_path.as_bytes(),
            body.as_bytes(),
            body_text_version.as_bytes(),
        ],
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexSignature<'a> {
    /// Runtime path that produced vectors (`local-hash`, `glm-cloud-interim`, …).
    pub runtime_path: &'a str,
    pub embedding_family: &'a str,
    pub embedding_revision: &'a str,
    pub dimensions: usize,
    pub normalized: bool,
    pub chunking_version: &'a str,
    /// NFC (or other) version applied to chunk bodies before hashing/embedding.
    pub body_text_version: &'a str,
    /// Query-side normalization version (e.g. accent folding for FTS/match).
    pub query_normalization_version: &'a str,
}

impl IndexSignature<'_> {
    pub fn digest(&self) -> String {
        digest(
            "index",
            &[
                self.runtime_path.as_bytes(),
                self.embedding_family.as_bytes(),
                self.embedding_revision.as_bytes(),
                &(self.dimensions as u64).to_be_bytes(),
                &[u8::from(self.normalized)],
                self.chunking_version.as_bytes(),
                self.body_text_version.as_bytes(),
                self.query_normalization_version.as_bytes(),
            ],
        )
    }
}

/// Compatibility only for existing desktop cache IDs; server code must use SHA-256 IDs.
pub fn legacy_desktop_hash(value: &str) -> String {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::{
        chunk_identity, digest, document_identity, IndexSignature, BODY_TEXT_VERSION,
        DEFAULT_CHUNKING_VERSION, QUERY_NORMALIZATION_VERSION, RUNTIME_LOCAL_HASH,
    };

    #[test]
    fn length_delimited_fields_are_not_ambiguous() {
        assert_ne!(
            digest("test", &[b"ab", b"c"]),
            digest("test", &[b"a", b"bc"])
        );
    }

    #[test]
    fn unicode_order_and_version_are_stable() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/identity-v1.json")).unwrap();
        assert_eq!(fixture["version"].as_u64().unwrap(), 2);
        let document = document_identity(
            fixture["document"]["sourceRel"].as_str().unwrap(),
            fixture["document"]["contentSha256"].as_str().unwrap(),
        );
        assert_eq!(document, fixture["document"]["identity"].as_str().unwrap());
        let chunk = chunk_identity(
            &document,
            fixture["chunk"]["versionId"].as_str().unwrap(),
            fixture["chunk"]["ordinal"].as_u64().unwrap(),
            fixture["chunk"]["headingPath"].as_str().unwrap(),
            fixture["chunk"]["body"].as_str().unwrap(),
            fixture["chunk"]["bodyTextVersion"].as_str().unwrap(),
        );
        assert_eq!(chunk, fixture["chunk"]["identity"].as_str().unwrap());
        assert_ne!(
            chunk,
            chunk_identity(
                &document,
                "version-other",
                7,
                "Chương I > Điều 2",
                "Nội dung tiếng Việt",
                BODY_TEXT_VERSION
            )
        );
        assert_ne!(
            chunk,
            chunk_identity(
                &document,
                fixture["chunk"]["versionId"].as_str().unwrap(),
                8,
                "Chương I > Điều 2",
                "Nội dung tiếng Việt",
                BODY_TEXT_VERSION
            )
        );
    }

    #[test]
    fn index_signature_covers_every_compatibility_dimension() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/identity-v1.json")).unwrap();
        let signature = IndexSignature {
            runtime_path: fixture["index"]["runtimePath"].as_str().unwrap(),
            embedding_family: fixture["index"]["embeddingFamily"].as_str().unwrap(),
            embedding_revision: fixture["index"]["embeddingRevision"].as_str().unwrap(),
            dimensions: fixture["index"]["dimensions"].as_u64().unwrap() as usize,
            normalized: fixture["index"]["normalized"].as_bool().unwrap(),
            chunking_version: fixture["index"]["chunkingVersion"].as_str().unwrap(),
            body_text_version: fixture["index"]["bodyTextVersion"].as_str().unwrap(),
            query_normalization_version: fixture["index"]["queryNormalizationVersion"]
                .as_str()
                .unwrap(),
        };
        assert_eq!(
            signature.digest(),
            fixture["index"]["signature"].as_str().unwrap()
        );
        assert_eq!(signature.chunking_version, DEFAULT_CHUNKING_VERSION);
        assert_eq!(signature.body_text_version, BODY_TEXT_VERSION);
        assert_eq!(
            signature.query_normalization_version,
            QUERY_NORMALIZATION_VERSION
        );
        assert_eq!(signature.runtime_path, RUNTIME_LOCAL_HASH);
        assert_ne!(
            signature.digest(),
            IndexSignature {
                dimensions: 768,
                ..signature.clone()
            }
            .digest()
        );
        assert_ne!(
            signature.digest(),
            IndexSignature {
                runtime_path: "glm-cloud-interim",
                ..signature.clone()
            }
            .digest()
        );
    }
}
