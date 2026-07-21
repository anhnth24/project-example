//! Pure markdown chunk preparation for indexing.

use fileconv_core::chunk::{chunk_markdown, locate_chunk_text};
use fileconv_core::intelligence::page_before;
use fileconv_knowledge::citation::infer_source_anchor;
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
    /// Trang PDF (marker `<!-- Page N -->`) chứa chunk, nếu suy được.
    pub page: Option<i32>,
    /// Slide PPTX suy từ heading "Slide N", nếu có.
    pub slide: Option<i32>,
    /// Sheet XLSX suy từ heading, nếu tài liệu là xlsx.
    pub sheet: Option<String>,
    /// Byte offset của body trong Markdown gốc — dùng cho citation anchor.
    pub span_start: i32,
    pub span_end: i32,
}

/// Chuẩn bị chunk cho indexing. `document_format` là đuôi file nguồn
/// (vd "pdf"/"pptx"/"xlsx") dùng để suy sheet; để trống nếu không rõ.
///
/// Ordinal và chunk identity KHÔNG phụ thuộc `document_format` nên metadata
/// nguồn (page/slide/sheet/span) là bổ sung thuần, không đổi index signature.
pub fn prepare_chunks(
    document_id: Uuid,
    version_id: Uuid,
    markdown: &str,
    document_format: &str,
) -> Vec<PreparedChunk> {
    let document_id = document_id.to_string();
    let version_id = version_id.to_string();
    let mut cursor = 0usize;
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

            // Định vị body trong Markdown gốc để lấy byte span + trang.
            // Cùng `locate_chunk_text` với `fileconv_core::intelligence::build_corpus`
            // (khớp LF/CRLF, UTF-8-safe) để anchor server khớp desktop.
            let (start, end) = match locate_chunk_text(markdown, cursor, &chunk.text) {
                Some(span) => span,
                None => (cursor, cursor),
            };
            cursor = end;

            let page = page_before(markdown, start);
            let anchor = infer_source_anchor(document_format, &chunk.heading, page, start, end);

            PreparedChunk {
                ordinal,
                heading_path,
                heading_joined: chunk.heading,
                body: chunk.text,
                chunk_identity: identity,
                page: anchor.page.and_then(|value| i32::try_from(value).ok()),
                slide: anchor.slide.and_then(|value| i32::try_from(value).ok()),
                sheet: anchor.sheet,
                span_start: i32::try_from(anchor.start).unwrap_or(i32::MAX),
                span_end: i32::try_from(anchor.end).unwrap_or(i32::MAX),
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
        let first = prepare_chunks(document_id, version_id, markdown, "");
        let second = prepare_chunks(document_id, version_id, markdown, "");
        assert_eq!(first, second);
        assert_eq!(first[1].heading_joined, "Chương I > Điều 1");
        assert_eq!(first[1].heading_path, vec!["Chương I", "Điều 1"]);
        assert_eq!(first[1].chunk_identity.len(), 64);
    }

    #[test]
    fn prepare_chunks_returns_empty_for_empty_markdown() {
        assert!(prepare_chunks(Uuid::new_v4(), Uuid::new_v4(), " \n\t ", "").is_empty());
    }

    #[test]
    fn prepare_chunks_captures_page_marker_and_span() {
        let document_id = Uuid::new_v4();
        let version_id = Uuid::new_v4();
        let markdown = "<!-- Page 7 -->\n\n# Thanh toán\n\nCho phép thanh toán QR.";
        let chunks = prepare_chunks(document_id, version_id, markdown, "pdf");
        let body_chunk = chunks
            .iter()
            .find(|chunk| chunk.body.contains("thanh toán QR"))
            .expect("body chunk present");
        assert_eq!(body_chunk.page, Some(7));
        assert!(body_chunk.span_end > body_chunk.span_start);
        let quoted = &markdown[body_chunk.span_start as usize..body_chunk.span_end as usize];
        assert!(quoted.contains("thanh toán QR"));
    }

    #[test]
    fn prepare_chunks_infers_slide_and_sheet_from_heading() {
        let document_id = Uuid::new_v4();
        let version_id = Uuid::new_v4();
        let slide_md = "# Phụ lục > Slide 12\n\nNội dung slide.";
        let slide = prepare_chunks(document_id, version_id, slide_md, "pptx");
        assert_eq!(slide[0].slide, Some(12));

        let sheet_md = "# Báo cáo > Quý I\n\nDoanh thu.";
        let sheet = prepare_chunks(document_id, version_id, sheet_md, "xlsx");
        assert_eq!(sheet[0].sheet.as_deref(), Some("Quý I"));
        // Non-xlsx không suy sheet.
        let not_sheet = prepare_chunks(document_id, version_id, sheet_md, "pdf");
        assert_eq!(not_sheet[0].sheet, None);
    }

    #[test]
    fn prepare_chunks_multiline_crlf_span_matches_exact_quoted_content() {
        let document_id = Uuid::new_v4();
        let version_id = Uuid::new_v4();
        let markdown = "# Tiếng Việt\r\n\r\nHệ thống phải giữ dấu.\r\nDòng hai vẫn khớp.\r\n";
        let chunks = prepare_chunks(document_id, version_id, markdown, "");
        let body = chunks
            .iter()
            .find(|chunk| chunk.body.contains("giữ dấu"))
            .expect("body chunk");
        let start = body.span_start as usize;
        let end = body.span_end as usize;
        assert!(markdown.is_char_boundary(start));
        assert!(markdown.is_char_boundary(end));
        assert_eq!(
            &markdown[start..end],
            "Hệ thống phải giữ dấu.\r\nDòng hai vẫn khớp."
        );
        // Body giữ bản LF từ chunker; span trỏ đúng byte CRLF trên nguồn.
        assert_eq!(body.body, "Hệ thống phải giữ dấu.\nDòng hai vẫn khớp.");
    }
}
