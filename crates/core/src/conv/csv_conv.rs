//! CSV → bảng Markdown. Cải tiến so với markitdown-rs (chỉ nối dấu phẩy): xuất
//! bảng Markdown đúng chuẩn, hàng đầu làm header.

use std::path::Path;

use super::{esc_cell, fail};
use crate::ConvertError;

pub fn to_markdown(path: &Path) -> Result<String, ConvertError> {
    let raw = std::fs::read(path).map_err(fail)?;
    // Bỏ BOM UTF-8 nếu có.
    let bytes = raw.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&raw[..]);
    // Giải mã: UTF-8 chuẩn; non-UTF8 → thử TCVN3 (bảng mã VN cũ, ra chữ Việt đúng
    // thay vì rác); còn lại lossy để KHÔNG bỏ mất dòng.
    let text = crate::viet_legacy::decode_text(bytes);
    // Tự nhận dấu phân tách: , ; tab | (Excel tiếng Việt hay xuất dấu ;).
    let delim = sniff_delimiter(&text);

    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delim)
        .from_reader(text.as_bytes());

    let rows: Vec<Vec<String>> = rdr
        .records()
        .filter_map(|r| r.ok())
        .map(|rec| rec.iter().map(esc_cell).collect())
        .collect();

    Ok(rows_to_md_table(&rows))
}

/// Đoán dấu phân tách từ dòng dữ liệu đầu tiên (chọn ký tự xuất hiện nhiều nhất).
fn sniff_delimiter(text: &str) -> u8 {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    [b',', b';', b'\t', b'|']
        .into_iter()
        .filter(|&d| line.bytes().any(|b| b == d))
        .max_by_key(|&d| line.bytes().filter(|&b| b == d).count())
        .unwrap_or(b',')
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MergeRange {
    pub start_row: usize,
    pub start_col: usize,
    pub end_row: usize,
    pub end_col: usize,
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "<br>")
}

pub(crate) fn rows_to_html_table(rows: &[Vec<String>], merges: &[MergeRange]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let columns = rows.iter().map(Vec::len).max().unwrap_or_default();
    if columns == 0 {
        return String::new();
    }
    let mut covered = vec![vec![false; columns]; rows.len()];
    let mut origins = std::collections::HashMap::new();
    for merge in merges {
        if merge.start_row >= rows.len()
            || merge.start_col >= columns
            || merge.end_row < merge.start_row
            || merge.end_col < merge.start_col
        {
            continue;
        }
        let end_row = merge.end_row.min(rows.len() - 1);
        let end_col = merge.end_col.min(columns - 1);
        origins.insert(
            (merge.start_row, merge.start_col),
            (end_row - merge.start_row + 1, end_col - merge.start_col + 1),
        );
        for row in merge.start_row..=end_row {
            for column in merge.start_col..=end_col {
                if (row, column) != (merge.start_row, merge.start_col) {
                    covered[row][column] = true;
                }
            }
        }
    }

    let mut output = String::from("<table>\n");
    for (row_index, row) in rows.iter().enumerate() {
        output.push_str("  <tr>");
        for column in 0..columns {
            if covered[row_index][column] {
                continue;
            }
            let tag = if row_index == 0 { "th" } else { "td" };
            let (rowspan, colspan) = origins.get(&(row_index, column)).copied().unwrap_or((1, 1));
            output.push('<');
            output.push_str(tag);
            if rowspan > 1 {
                output.push_str(&format!(" rowspan=\"{rowspan}\""));
            }
            if colspan > 1 {
                output.push_str(&format!(" colspan=\"{colspan}\""));
            }
            output.push('>');
            output.push_str(&html_escape(
                row.get(column).map(String::as_str).unwrap_or_default(),
            ));
            output.push_str("</");
            output.push_str(tag);
            output.push('>');
        }
        output.push_str("</tr>\n");
    }
    output.push_str("</table>\n");
    output
}

#[cfg(test)]
mod tests {
    use super::{rows_to_html_table, sniff_delimiter, MergeRange};

    #[test]
    fn sniff_picks_delimiter() {
        assert_eq!(sniff_delimiter("Ten;Tuoi;Diachi"), b';');
        assert_eq!(sniff_delimiter("a,b,c"), b',');
        assert_eq!(sniff_delimiter("a\tb\tc"), b'\t');
        assert_eq!(sniff_delimiter("a|b"), b'|');
        assert_eq!(sniff_delimiter("nodelim"), b','); // mặc định
    }

    #[test]
    fn html_table_preserves_multiline_and_merged_cells() {
        let rows = vec![
            vec!["Tiêu đề".into(), String::new(), "Khác".into()],
            vec!["A\nB".into(), "C".into(), "D".into()],
        ];
        let html = rows_to_html_table(
            &rows,
            &[MergeRange {
                start_row: 0,
                start_col: 0,
                end_row: 0,
                end_col: 1,
            }],
        );
        assert!(html.contains("<th colspan=\"2\">Tiêu đề</th>"));
        assert!(html.contains("<td>A<br>B</td>"));
        assert!(!html.contains("<script"));
    }
}
