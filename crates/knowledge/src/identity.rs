use sha2::{Digest, Sha256};
use std::fmt::Write;

pub const IDENTITY_VERSION: u16 = 1;

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

pub fn chunk_identity(
    document_id: &str,
    ordinal: u64,
    heading_path: &str,
    body: &str,
    text_version: &str,
) -> String {
    digest(
        "chunk",
        &[
            document_id.as_bytes(),
            &ordinal.to_be_bytes(),
            heading_path.as_bytes(),
            body.as_bytes(),
            text_version.as_bytes(),
        ],
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexSignature<'a> {
    pub embedding_family: &'a str,
    pub embedding_revision: &'a str,
    pub dimensions: usize,
    pub normalized: bool,
    pub chunking_version: &'a str,
    pub text_version: &'a str,
}

impl IndexSignature<'_> {
    pub fn digest(&self) -> String {
        digest(
            "index",
            &[
                self.embedding_family.as_bytes(),
                self.embedding_revision.as_bytes(),
                &(self.dimensions as u64).to_be_bytes(),
                &[u8::from(self.normalized)],
                self.chunking_version.as_bytes(),
                self.text_version.as_bytes(),
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
    use super::{chunk_identity, digest, document_identity, IndexSignature};

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
        let document = document_identity(
            fixture["document"]["sourceRel"].as_str().unwrap(),
            fixture["document"]["contentSha256"].as_str().unwrap(),
        );
        assert_eq!(document, fixture["document"]["identity"].as_str().unwrap());
        let chunk = chunk_identity(
            &document,
            fixture["chunk"]["ordinal"].as_u64().unwrap(),
            fixture["chunk"]["headingPath"].as_str().unwrap(),
            fixture["chunk"]["body"].as_str().unwrap(),
            fixture["chunk"]["textVersion"].as_str().unwrap(),
        );
        assert_eq!(chunk, fixture["chunk"]["identity"].as_str().unwrap());
        assert_ne!(
            chunk,
            chunk_identity(
                &document,
                8,
                "Chương I > Điều 2",
                "Nội dung tiếng Việt",
                "nfc-v1"
            )
        );
    }

    #[test]
    fn index_signature_covers_every_compatibility_dimension() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/identity-v1.json")).unwrap();
        let signature = IndexSignature {
            embedding_family: fixture["index"]["embeddingFamily"].as_str().unwrap(),
            embedding_revision: fixture["index"]["embeddingRevision"].as_str().unwrap(),
            dimensions: fixture["index"]["dimensions"].as_u64().unwrap() as usize,
            normalized: fixture["index"]["normalized"].as_bool().unwrap(),
            chunking_version: fixture["index"]["chunkingVersion"].as_str().unwrap(),
            text_version: fixture["index"]["textVersion"].as_str().unwrap(),
        };
        assert_eq!(
            signature.digest(),
            fixture["index"]["signature"].as_str().unwrap()
        );
        assert_ne!(
            signature.digest(),
            IndexSignature {
                dimensions: 768,
                ..signature.clone()
            }
            .digest()
        );
    }
}
