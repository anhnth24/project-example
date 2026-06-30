//! CSV → bảng Markdown. Cải tiến so với markitdown-rs (chỉ nối dấu phẩy): xuất
//! bảng Markdown đúng chuẩn, hàng đầu làm header.

use std::path::Path;

use super::{esc_cell, fail};
use crate::ConvertError;

pub fn to_markdown(path: &Path) -> Result<String, ConvertError> {
    let raw = std::fs::read(path).map_err(fail)?;
    // Bỏ BOM UTF-8 nếu có.
    let bytes = raw.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&raw[..]);
    // Giải mã: UTF-8 chuẩn; nếu không hợp lệ (CSV legacy Windows-1252/1258…) thì
    // dùng lossy để KHÔNG bỏ mất dòng (trước đây dòng non-UTF8 bị loại sạch).
    let text = match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => String::from_utf8_lossy(bytes).into_owned(),
    };
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

#[cfg(test)]
mod tests {
    use super::sniff_delimiter;

    #[test]
    fn sniff_picks_delimiter() {
        assert_eq!(sniff_delimiter("Ten;Tuoi;Diachi"), b';');
        assert_eq!(sniff_delimiter("a,b,c"), b',');
        assert_eq!(sniff_delimiter("a\tb\tc"), b'\t');
        assert_eq!(sniff_delimiter("a|b"), b'|');
        assert_eq!(sniff_delimiter("nodelim"), b','); // mặc định
    }
}
