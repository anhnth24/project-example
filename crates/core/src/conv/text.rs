//! Plain text and legacy Vietnamese text → UTF-8 Markdown-compatible text.

use std::path::Path;

use super::fail;
use crate::ConvertError;

pub fn to_markdown(path: &Path) -> Result<String, ConvertError> {
    let bytes = std::fs::read(path).map_err(fail)?;
    let bytes = bytes
        .strip_prefix(&[0xEF, 0xBB, 0xBF])
        .unwrap_or(bytes.as_slice());
    Ok(crate::viet_legacy::decode_text(bytes))
}
