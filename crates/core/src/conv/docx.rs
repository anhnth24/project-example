//! DOCX → Markdown.
//!
//! Cải tiến so với markitdown-rs (chỉ nối text trơn):
//!   - Phát hiện **heading** qua style ("Heading1".."Heading6", "Title") → `#`..`######`.
//!   - Đoạn thuộc danh sách (numbering) → `- `.
//!   - Bảng → bảng Markdown đúng chuẩn (escape `|`).

use std::path::Path;

use docx_rust::document::{BodyContent, TableCellContent, TableRowContent};
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
                let text: String = p.iter_text().map(|c| c.as_ref()).collect();
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
                    md.push_str(&"#".repeat(level));
                    md.push(' ');
                    md.push_str(text);
                    md.push_str("\n\n");
                } else if is_list {
                    md.push_str("- ");
                    md.push_str(text);
                    md.push('\n');
                } else {
                    md.push_str(text);
                    md.push_str("\n\n");
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
                                for t in p.iter_text() {
                                    s.push_str(t.as_ref());
                                }
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
