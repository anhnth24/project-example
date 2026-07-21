//! Chia Markdown thành CHUNK cho RAG/embedding (gap so với Docling HybridChunker /
//! Marker "chunks" — xem bench/RESEARCH_COMPETITORS.md).
//!
//! Chiến lược: chia theo **heading** (giữ ngữ cảnh tiêu đề cha), section dài quá
//! `max_chars` thì chia tiếp theo đoạn trống (paragraph boundary) — không cắt giữa câu.

/// Một chunk kết quả.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Chunk {
    pub index: usize,
    /// Đường dẫn heading dẫn tới chunk (vd "Chương I > Điều 1").
    pub heading: String,
    pub text: String,
    pub chars: usize,
}

// serde chỉ dùng cho Serialize của Chunk — thêm dep nhẹ qua serde_json đã có.
use serde::ser::Serialize as _;

/// Chuẩn hoá mọi kiểu xuống dòng (`\r\n` / `\r`) về `\n` trước khi chunk.
/// `chunk_markdown` luôn làm việc trên LF; caller định vị lại span trên nguồn gốc
/// bằng [`locate_chunk_text`].
pub fn normalize_newlines(md: &str) -> std::borrow::Cow<'_, str> {
    if !md.as_bytes().contains(&b'\r') {
        return std::borrow::Cow::Borrowed(md);
    }
    std::borrow::Cow::Owned(md.replace("\r\n", "\n").replace('\r', "\n"))
}

/// Clamp `offset` xuống char boundary gần nhất bên trái (hoặc 0).
pub fn clamp_to_char_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

/// Định vị `needle` (chunk text đã chuẩn LF) trong `haystack` gốc kể từ `cursor`.
///
/// `\n` trong needle khớp `\n`, `\r\n` hoặc `\r` trong haystack. Trả về
/// `(start, end)` byte offset UTF-8-safe trên haystack, hoặc `None` nếu không thấy.
pub fn locate_chunk_text(haystack: &str, cursor: usize, needle: &str) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return None;
    }
    let cursor = clamp_to_char_boundary(haystack, cursor.min(haystack.len()));

    // Fast path: khớp nguyên văn (tài liệu LF).
    if let Some(relative) = haystack[cursor..].find(needle) {
        let start = cursor + relative;
        let end = start + needle.len();
        if haystack.is_char_boundary(start) && haystack.is_char_boundary(end) {
            return Some((start, end));
        }
    }

    // Slow path: needle LF khớp haystack CRLF/CR.
    let hay_bytes = haystack.as_bytes();
    let needle_bytes = needle.as_bytes();
    let mut start = cursor;
    while start < hay_bytes.len() {
        if let Some(end) = match_lf_needle_at(hay_bytes, start, needle_bytes) {
            if haystack.is_char_boundary(start) && haystack.is_char_boundary(end) {
                return Some((start, end));
            }
        }
        start = next_char_boundary(haystack, start + 1);
    }
    None
}

fn next_char_boundary(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while offset < text.len() && !text.is_char_boundary(offset) {
        offset += 1;
    }
    offset
}

fn match_lf_needle_at(haystack: &[u8], start: usize, needle: &[u8]) -> Option<usize> {
    let mut hi = start;
    let mut ni = 0usize;
    while ni < needle.len() {
        if needle[ni] == b'\n' {
            if hi >= haystack.len() {
                return None;
            }
            match haystack[hi] {
                b'\n' => hi += 1,
                b'\r' => {
                    hi += 1;
                    if hi < haystack.len() && haystack[hi] == b'\n' {
                        hi += 1;
                    }
                }
                _ => return None,
            }
            ni += 1;
            continue;
        }
        if hi >= haystack.len() || haystack[hi] != needle[ni] {
            return None;
        }
        hi += 1;
        ni += 1;
    }
    Some(hi)
}

/// Chia markdown thành chunks ≤ `max_chars` (xấp xỉ — đo theo ký tự).
pub fn chunk_markdown(md: &str, max_chars: usize) -> Vec<Chunk> {
    let md = normalize_newlines(md);
    let max_chars = max_chars.max(200); // sàn tối thiểu hợp lý
                                        // 1) Gom thành section theo heading.
    let mut sections: Vec<(Vec<String>, String)> = Vec::new(); // (heading-path, body)
    let mut path: Vec<(usize, String)> = Vec::new(); // (level, title)
    let mut body = String::new();

    let flush =
        |sections: &mut Vec<(Vec<String>, String)>, path: &[(usize, String)], body: &mut String| {
            if !body.trim().is_empty() {
                sections.push((
                    path.iter().map(|(_, t)| t.clone()).collect(),
                    std::mem::take(body),
                ));
            } else {
                body.clear();
            }
        };

    for line in md.lines() {
        let trimmed = line.trim_start();
        let hashes = trimmed.bytes().take_while(|&b| b == b'#').count();
        if hashes >= 1 && hashes <= 6 && trimmed.as_bytes().get(hashes) == Some(&b' ') {
            flush(&mut sections, &path, &mut body);
            let title = trimmed[hashes + 1..].trim().to_string();
            while matches!(path.last(), Some((l, _)) if *l >= hashes) {
                path.pop();
            }
            path.push((hashes, title));
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    flush(&mut sections, &path, &mut body);

    // 2) Section dài → chia theo đoạn trống.
    let mut chunks = Vec::new();
    for (hpath, text) in sections {
        let heading = hpath.join(" > ");
        let text = text.trim();
        if text.chars().count() <= max_chars {
            push_chunk(&mut chunks, &heading, text);
            continue;
        }
        let mut cur = String::new();
        for para in text.split("\n\n") {
            let plen = para.chars().count();
            let clen = cur.chars().count();
            if clen > 0 && clen + plen + 2 > max_chars {
                push_chunk(&mut chunks, &heading, cur.trim());
                cur.clear();
            }
            // Đoạn đơn lẻ vẫn dài hơn max → cắt cứng theo ký tự (hiếm: bảng khổng lồ).
            if plen > max_chars {
                let mut it = para.chars().peekable();
                while it.peek().is_some() {
                    let piece: String = it.by_ref().take(max_chars).collect();
                    push_chunk(&mut chunks, &heading, piece.trim());
                }
            } else {
                if !cur.is_empty() {
                    cur.push_str("\n\n");
                }
                cur.push_str(para);
            }
        }
        if !cur.trim().is_empty() {
            push_chunk(&mut chunks, &heading, cur.trim());
        }
    }
    chunks
}

fn push_chunk(chunks: &mut Vec<Chunk>, heading: &str, text: &str) {
    if text.is_empty() {
        return;
    }
    chunks.push(Chunk {
        index: chunks.len(),
        heading: heading.to_string(),
        text: text.to_string(),
        chars: text.chars().count(),
    });
}

/// JSON hoá danh sách chunk.
pub fn chunks_json(chunks: &[Chunk]) -> String {
    let mut buf = Vec::new();
    let mut ser = serde_json::Serializer::new(&mut buf);
    chunks.serialize(&mut ser).ok();
    String::from_utf8(buf).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_by_heading_with_path() {
        let md = "# Chương I\n\nMở đầu.\n\n## Điều 1\n\nNội dung điều 1.\n\n## Điều 2\n\nNội dung điều 2.\n";
        let c = chunk_markdown(md, 1000);
        assert_eq!(c.len(), 3);
        assert_eq!(c[0].heading, "Chương I");
        assert_eq!(c[1].heading, "Chương I > Điều 1");
        assert_eq!(c[2].heading, "Chương I > Điều 2");
        assert!(c[1].text.contains("Nội dung điều 1"));
    }

    #[test]
    fn long_section_splits_at_paragraphs() {
        let para = "x".repeat(150);
        let md = format!("# A\n\n{para}\n\n{para}\n\n{para}\n");
        let c = chunk_markdown(&md, 320);
        assert!(c.len() >= 2, "phải chia nhỏ, got {}", c.len());
        assert!(c.iter().all(|k| k.chars <= 320));
        assert!(c.iter().all(|k| k.heading == "A"));
    }

    #[test]
    fn heading_level_pops_correctly() {
        let md = "# A\n\nbody a\n\n## B\n\nbody b\n\n# C\n\nbody c\n";
        let c = chunk_markdown(md, 1000);
        assert_eq!(c[2].heading, "C"); // không còn dính "A >"
    }

    #[test]
    fn locate_chunk_text_matches_multiline_crlf_exactly() {
        let md = "# Tiếng Việt\r\n\r\nHệ thống phải giữ dấu.\r\nDòng hai vẫn khớp.\r\n";
        let chunks = chunk_markdown(md, 2_000);
        assert_eq!(chunks.len(), 1);
        let (start, end) = locate_chunk_text(md, 0, &chunks[0].text).expect("span");
        assert_eq!(
            &md[start..end],
            "Hệ thống phải giữ dấu.\r\nDòng hai vẫn khớp."
        );
        assert_eq!(normalize_newlines(&md[start..end]), chunks[0].text);
    }

    #[test]
    fn locate_chunk_text_is_utf8_safe_for_vietnamese() {
        let md = "# Mục\n\nNội dung có chữ ệ và ư.\n";
        let chunks = chunk_markdown(md, 2_000);
        let (start, end) = locate_chunk_text(md, 0, &chunks[0].text).expect("span");
        assert!(md.is_char_boundary(start));
        assert!(md.is_char_boundary(end));
        assert_eq!(&md[start..end], chunks[0].text.as_str());
    }

    #[test]
    fn clamp_to_char_boundary_backs_up_from_mid_glyph() {
        let text = "ệ";
        assert_eq!(text.len(), 3);
        assert_eq!(clamp_to_char_boundary(text, 1), 0);
        assert_eq!(clamp_to_char_boundary(text, 3), 3);
    }
}
