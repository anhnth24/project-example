//! CSV → bảng Markdown. Cải tiến so với markitdown-rs (chỉ nối dấu phẩy): xuất
//! bảng Markdown đúng chuẩn, hàng đầu làm header.

use std::path::Path;

use super::{esc_cell, fail};
use crate::ConvertError;

pub fn to_markdown(path: &Path) -> Result<String, ConvertError> {
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_path(path)
        .map_err(fail)?;

    let rows: Vec<Vec<String>> = rdr
        .records()
        .filter_map(|r| r.ok())
        .map(|rec| rec.iter().map(esc_cell).collect())
        .collect();

    Ok(rows_to_md_table(&rows))
}

/// Dựng bảng Markdown từ các hàng (hàng 0 là header). Chuẩn hoá số cột.
pub(crate) fn rows_to_md_table(rows: &[Vec<String>]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if cols == 0 {
        return String::new();
    }
    fn cell(row: &[String], i: usize) -> &str {
        row.get(i).map(|s| s.as_str()).unwrap_or("")
    }

    let mut out = String::new();
    // header
    out.push('|');
    for i in 0..cols {
        out.push_str(&format!(" {} |", cell(&rows[0], i)));
    }
    out.push('\n');
    // separator
    out.push('|');
    for _ in 0..cols {
        out.push_str(" --- |");
    }
    out.push('\n');
    // body
    for row in rows.iter().skip(1) {
        out.push('|');
        for i in 0..cols {
            out.push_str(&format!(" {} |", cell(row, i)));
        }
        out.push('\n');
    }
    out
}
