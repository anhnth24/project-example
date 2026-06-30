//! DOCX → Markdown.
//!
//! Cải tiến so với markitdown-rs (chỉ nối text trơn bằng iter_text):
//!   - Duyệt từng run, xử lý `<w:br>`/`<w:cr>` → xuống dòng, `<w:tab>` → khoảng trắng
//!     → KHÔNG còn dính chữ kiểu "Chương INHỮNG QUY ĐỊNH CHUNG".
//!   - Lấy text trong hyperlink.
//!   - Phát hiện **heading** qua style ("Heading1".."Heading6", "Title") → `#`..`######`.
//!   - Đoạn thuộc danh sách (numbering) → `- `.
//!   - Bảng → bảng Markdown đúng chuẩn (escape `|`).

use std::path::Path;

use docx_rust::document::{
    BodyContent, Paragraph, ParagraphContent, Run, RunContent, TableCellContent, TableRowContent,
};
use docx_rust::DocxFile;

use super::{esc_cell, fail};
use crate::conv::csv_conv::rows_to_md_table;
use crate::ConvertError;

pub fn to_markdown(path: &Path) -> Result<String, ConvertError> {
    let docx = DocxFile::from_file(path).map_err(|e| fail(format!("{e:?}")))?;
    let doc = docx.parse().map_err(|e| fail(format!("{e:?}")))?;

    let mut md = String::new();
    for content in &doc.document.body.content {
        match content {
            BodyContent::Paragraph(p) => {
                let text = para_text(p);
                let text = text.trim();
                if text.is_empty() {
                    continue;
                }
                let style = p
                    .property
                    .as_ref()
                    .and_then(|pr| pr.style_id.as_ref())
                    .map(|s| s.value.as_ref());
                let is_list = p
                    .property
                    .as_ref()
                    .map(|pr| pr.numbering.is_some())
                    .unwrap_or(false);

                if let Some(level) = style.and_then(heading_level) {
                    // Heading nằm trên một dòng: gộp các break thành khoảng trắng.
                    let line = text.split('\n').map(str::trim).collect::<Vec<_>>().join(" ");
                    md.push_str(&"#".repeat(level));
                    md.push(' ');
                    md.push_str(line.trim());
                    md.push_str("\n\n");
                } else if is_list {
                    md.push_str("- ");
                    md.push_str(&text.replace('\n', " "));
                    md.push('\n');
                } else {
                    // Giữ các break trong đoạn thành dòng riêng.
                    for line in text.split('\n') {
                        let line = line.trim();
                        if !line.is_empty() {
                            md.push_str(line);
                            md.push('\n');
                        }
                    }
                    md.push('\n');
                }
            }
            BodyContent::Table(table) => {
                let mut rows: Vec<Vec<String>> = Vec::new();
                for row in &table.rows {
                    let mut cells: Vec<String> = Vec::new();
                    for cell in &row.cells {
                        if let TableRowContent::TableCell(tc) = cell {
                            let mut s = String::new();
                            for c in &tc.content {
                                let TableCellContent::Paragraph(p) = c;
                                s.push_str(&para_text(p));
                                s.push(' ');
                            }
                            cells.push(esc_cell(&s));
                        }
                    }
                    if !cells.is_empty() {
                        rows.push(cells);
                    }
                }
                md.push_str(&rows_to_md_table(&rows));
                md.push('\n');
            }
            _ => {}
        }
    }
    Ok(md)
}

/// Trích text của một đoạn: gồm run trực tiếp và run trong hyperlink.
/// Gộp các run liền kề CÙNG kiểu định dạng trước khi bọc markdown — Word hay tách
/// một câu/chữ thành nhiều run dù định dạng giống nhau (vd gắn `w:lang` khác cho
/// một ký tự có dấu), bọc `**`/`*` theo từng run riêng sẽ dính dấu lưng chừng chữ
/// (kiểu "một s**ố* *điều"). Gộp theo (bold, italic) trước rồi mới bọc 1 lần.
fn para_text(p: &Paragraph) -> String {
    let mut spans: Vec<(String, bool, bool)> = Vec::new();
    for c in &p.content {
        let run = match c {
            ParagraphContent::Run(r) => Some(r),
            ParagraphContent::Link(h) => h.content.as_ref(),
            _ => None,
        };
        let Some(run) = run else { continue };
        let (text, bold, italic) = run_raw(run);
        if text.is_empty() {
            continue;
        }
        match spans.last_mut() {
            Some(last) if last.1 == bold && last.2 == italic => last.0.push_str(&text),
            _ => spans.push((text, bold, italic)),
        }
    }

    let mut s = String::new();
    for (text, bold, italic) in spans {
        if !bold && !italic {
            s.push_str(&text);
            continue;
        }
        // Bọc từng dòng riêng (tránh dấu nhấn mạnh tràn qua xuống dòng).
        let wrapped = text
            .split('\n')
            .map(|line| wrap_emphasis(line, bold, italic))
            .collect::<Vec<_>>()
            .join("\n");
        s.push_str(&wrapped);
    }
    s
}

/// Trích text THÔ (chưa bọc markdown) + cờ bold/italic của một run.
/// Break → xuống dòng, tab → khoảng trắng (trước đây đẩy ký tự tab thô vào
/// markdown, gây lỗi hiển thị như "tên:␉………").
fn run_raw(run: &Run) -> (String, bool, bool) {
    let mut raw = String::new();
    for c in &run.content {
        match c {
            RunContent::Text(t) => raw.push_str(t.text.as_ref()),
            RunContent::Break(_) | RunContent::CarriageReturn(_) => raw.push('\n'),
            RunContent::Tab(_) | RunContent::PTab(_) => raw.push(' '),
            RunContent::NoBreakHyphen(_) => raw.push('-'),
            _ => {}
        }
    }

    let prop = run.property.as_ref();
    let bold = prop
        .and_then(|p| p.bold.as_ref())
        .map(|b| b.value != Some(false))
        .unwrap_or(false);
    let italic = prop
        .and_then(|p| p.italics.as_ref())
        .map(|i| i.value != Some(false))
        .unwrap_or(false);
    (raw, bold, italic)
}

/// Bọc `**`/`*`/`***` quanh phần chữ thật, giữ khoảng trắng đầu/cuối ở NGOÀI dấu
/// nhấn mạnh (CommonMark coi `** text**` là không hợp lệ nếu có khoảng trắng sát dấu).
fn wrap_emphasis(text: &str, bold: bool, italic: bool) -> String {
    let core = text.trim();
    if core.is_empty() {
        return text.to_string();
    }
    let lead = &text[..text.len() - text.trim_start().len()];
    let trail = &text[text.trim_end().len()..];
    let marker = match (bold, italic) {
        (true, true) => "***",
        (true, false) => "**",
        (false, true) => "*",
        (false, false) => return text.to_string(),
    };
    format!("{lead}{marker}{core}{marker}{trail}")
}

/// Map style id → cấp heading. "Title"→1, "Heading1".."Heading6"→1..6.
fn heading_level(style: &str) -> Option<usize> {
    let s = style.to_ascii_lowercase().replace([' ', '-', '_'], "");
    if s == "title" {
        return Some(1);
    }
    s.strip_prefix("heading")
        .and_then(|rest| rest.parse::<usize>().ok())
        .map(|n| n.clamp(1, 6))
}

#[cfg(test)]
mod tests {
    use super::wrap_emphasis;

    #[test]
    fn wraps_bold_only() {
        assert_eq!(wrap_emphasis("Điều 1.", true, false), "**Điều 1.**");
    }

    #[test]
    fn wraps_italic_only() {
        assert_eq!(wrap_emphasis("Căn cứ Luật…", false, true), "*Căn cứ Luật…*");
    }

    #[test]
    fn wraps_bold_italic() {
        assert_eq!(wrap_emphasis("abc", true, true), "***abc***");
    }

    #[test]
    fn keeps_surrounding_whitespace_outside_markers() {
        assert_eq!(wrap_emphasis("  Họ và tên  ", true, false), "  **Họ và tên**  ");
    }

    #[test]
    fn empty_or_whitespace_unchanged() {
        assert_eq!(wrap_emphasis("   ", true, true), "   ");
        assert_eq!(wrap_emphasis("", true, false), "");
    }

    #[test]
    fn no_emphasis_passthrough() {
        assert_eq!(wrap_emphasis("plain", false, false), "plain");
    }
}
