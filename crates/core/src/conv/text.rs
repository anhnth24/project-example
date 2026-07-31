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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_txt(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("fileconv_text_{}_{}.txt", name, std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn utf8_bom_stripped_keeps_vietnamese() {
        // Khóa strip UTF-8 BOM (EF BB BF) trước decode_text: artifact U+FEFF
        // không được lọt output; nội dung tiếng Việt vẫn còn.
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice("Xin chào Việt Nam".as_bytes());
        let path = temp_txt("utf8_bom", &bytes);
        let md = to_markdown(&path).expect("text convert");
        assert!(
            md.contains("Xin chào Việt Nam"),
            "expected Vietnamese body, got: {md:?}"
        );
        assert!(
            !md.contains('\u{feff}'),
            "UTF-8 BOM must be stripped, got: {md:?}"
        );
        let _ = std::fs::remove_file(path);
    }
}
