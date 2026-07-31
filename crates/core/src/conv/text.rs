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

    fn write_temp_text(suffix: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "fileconv_text_{}_{}.txt",
            suffix,
            std::process::id()
        ));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn utf8_bom_stripped_keeps_vietnamese() {
        // Khóa strip UTF-8 BOM (EF BB BF) trước decode_text: artifact U+FEFF
        // không được lọt output; nội dung tiếng Việt vẫn còn.
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice("Xin chào Việt Nam".as_bytes());
        let path = write_temp_text("utf8_bom_vi", &bytes);
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

    #[test]
    fn utf8_without_bom_decodes_vietnamese() {
        let body = "Tài liệu kiểm thử.";
        let path = write_temp_text("utf8_no_bom", body.as_bytes());
        let md = to_markdown(&path).expect("text convert");
        assert_eq!(md, body);
        assert!(!md.contains('\u{feff}'));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn utf8_bom_only_yields_empty_without_feff() {
        let path = write_temp_text("utf8_bom_only", &[0xEF, 0xBB, 0xBF]);
        let md = to_markdown(&path).expect("text convert");
        assert!(md.is_empty(), "expected empty body, got: {md:?}");
        assert!(!md.contains('\u{feff}'));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn strip_prefix_only_at_file_start() {
        // Chỉ cắt BOM ở byte 0; chuỗi EF BB BF giữa file vẫn decode thành U+FEFF.
        let mut bytes = b"start".to_vec();
        bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
        bytes.extend_from_slice(b"end");
        let path = write_temp_text("bom_in_middle", &bytes);
        let md = to_markdown(&path).expect("text convert");
        assert!(md.starts_with("start"));
        assert!(md.ends_with("end"));
        assert!(
            md.contains('\u{feff}'),
            "inner BOM bytes must not be stripped by strip_prefix, got: {md:?}"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn legacy_tcvn3_routes_through_decode_text() {
        // Cùng bytes TCVN3 mẫu như viet_legacy::tests (không BOM) — text path gọi decode_text.
        const TCVN3_SAMPLE: &[u8] = &[
            0x43, 0xE9, 0x6E, 0x67, 0x20, 0x68, 0xDF, 0x61, 0x20, 0x78, 0xB7, 0x20, 0x68, 0xE9,
            0x69, 0x20, 0x63, 0x68, 0xF1, 0x20, 0x6E, 0x67, 0x68, 0xDC, 0x61, 0x20, 0x56, 0x69,
            0xD6, 0x74, 0x20, 0x4E, 0x61, 0x6D,
        ];
        let path = write_temp_text("tcvn3_legacy", TCVN3_SAMPLE);
        let md = to_markdown(&path).expect("text convert");
        assert_eq!(md, "Cộng hòa xã hội chủ nghĩa Việt Nam");
        let _ = std::fs::remove_file(path);
    }
}
