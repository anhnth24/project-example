//! Các converter định dạng → Markdown. Mỗi module có `to_markdown(&Path) -> Result<String, ConvertError>`.

pub mod csv_conv;
pub mod docx;
pub mod html;
pub mod pdf;
pub mod pptx;
pub mod text;
pub mod xlsx;

use crate::ConvertError;

/// Helper: escape ký tự `|` trong ô bảng Markdown.
pub(crate) fn esc_cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ").trim().to_string()
}

/// Helper bọc lỗi io/parse.
pub(crate) fn fail<E: std::fmt::Display>(e: E) -> ConvertError {
    ConvertError::Failed(e.to_string())
}
