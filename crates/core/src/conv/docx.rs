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
fn para_text(p: &Paragraph) -> String {
    let mut s = String::new();
    for c in &p.content {
        match c {
            ParagraphContent::Run(r) => s.push_str(&run_text(r)),
            ParagraphContent::Link(h) => {
                if let Some(r) = &h.content {
                    s.push_str(&run_text(r));
                }
            }
            _ => {}
        }
    }
    s
}

/// Trích text của một run, biến break/tab thành ký tự khoảng trắng phù hợp.
fn run_text(run: &Run) -> String {
    let mut s = String::new();
    for c in &run.content {
        match c {
            RunContent::Text(t) => s.push_str(t.text.as_ref()),
            RunContent::Break(_) | RunContent::CarriageReturn(_) => s.push('\n'),
            RunContent::Tab(_) | RunContent::PTab(_) => s.push('\t'),
            RunContent::NoBreakHyphen(_) => s.push('-'),
            _ => {}
        }
    }
    s
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
