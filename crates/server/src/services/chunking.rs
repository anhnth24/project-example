//! Pure markdown chunk preparation for indexing.

use fileconv_core::chunk::chunk_markdown;
use fileconv_knowledge::identity::{chunk_identity, BODY_TEXT_VERSION};
use uuid::Uuid;

const CHUNK_MAX_CHARS: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedChunk {
    pub ordinal: i32,
    pub heading_path: Vec<String>,
    pub heading_joined: String,
    pub body: String,
    pub chunk_identity: String,
}

pub fn prepare_chunks(document_id: Uuid, version_id: Uuid, markdown: &str) -> Vec<PreparedChunk> {
    let document_id = document_id.to_string();
    let version_id = version_id.to_string();
    chunk_markdown(markdown, CHUNK_MAX_CHARS)
        .into_iter()
        .map(|chunk| {
            let heading_path = if chunk.heading.is_empty() {
                Vec::new()
            } else {
                chunk.heading.split(" > ").map(str::to_string).collect()
            };
            let ordinal = i32::try_from(chunk.index).expect("chunk index fits in i32");
            let identity = chunk_identity(
                &document_id,
                &version_id,
                chunk.index as u64,
                &chunk.heading,
                &chunk.text,
                BODY_TEXT_VERSION,
            );
            PreparedChunk {
                ordinal,
                heading_path,
                heading_joined: chunk.heading,
                body: chunk.text,
                chunk_identity: identity,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_chunks_splits_heading_path_and_identity_is_deterministic() {
        let document_id = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
        let version_id = Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap();
        let markdown = "# Chương I\n\nMở đầu.\n\n## Điều 1\n\nNội dung điều 1.";
        let first = prepare_chunks(document_id, version_id, markdown);
        let second = prepare_chunks(document_id, version_id, markdown);
        assert_eq!(first, second);
        assert_eq!(first[1].heading_joined, "Chương I > Điều 1");
        assert_eq!(first[1].heading_path, vec!["Chương I", "Điều 1"]);
        assert_eq!(first[1].chunk_identity.len(), 64);
    }

    #[test]
    fn prepare_chunks_returns_empty_for_empty_markdown() {
        assert!(prepare_chunks(Uuid::new_v4(), Uuid::new_v4(), " \n\t ").is_empty());
    }
}
