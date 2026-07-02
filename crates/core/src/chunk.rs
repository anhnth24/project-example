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

/// Chia markdown thành chunks ≤ `max_chars` (xấp xỉ — đo theo ký tự).
pub fn chunk_markdown(md: &str, max_chars: usize) -> Vec<Chunk> {
    let max_chars = max_chars.max(200); // sàn tối thiểu hợp lý
    // 1) Gom thành section theo heading.
    let mut sections: Vec<(Vec<String>, String)> = Vec::new(); // (heading-path, body)
    let mut path: Vec<(usize, String)> = Vec::new(); // (level, title)
    let mut body = String::new();

    let flush = |sections: &mut Vec<(Vec<String>, String)>, path: &[(usize, String)], body: &mut String| {
        if !body.trim().is_empty() {
            sections.push((path.iter().map(|(_, t)| t.clone()).collect(), std::mem::take(body)));
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
}
