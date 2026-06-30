//! HTML → Markdown.
//!
//! Dùng `htmd` (dựa trên html5ever) THAY cho `html2md` của markitdown-rs — sửa lỗi
//! `html2md` phình output khổng lồ (88 triệu ký tự) trên trang Wikipedia lớn.

use std::path::Path;

use super::fail;
use crate::ConvertError;

pub fn to_markdown(path: &Path) -> Result<String, ConvertError> {
    let bytes = std::fs::read(path).map_err(fail)?;
    let html = String::from_utf8_lossy(&bytes);
    // Bỏ <script>/<style> để không lọt mã JS/CSS vào Markdown.
    let converter = htmd::HtmlToMarkdown::builder()
        .skip_tags(vec!["script", "style", "noscript"])
        .build();
    converter.convert(&html).map_err(fail)
}
