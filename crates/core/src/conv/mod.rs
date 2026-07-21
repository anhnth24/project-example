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

/// Compatibility catch-all: unclassified io/parse failures stay [`ConvertError::Failed`].
/// Prefer the typed helpers below when the call site has clear evidence.
pub(crate) fn fail<E: std::fmt::Display>(e: E) -> ConvertError {
    ConvertError::Failed(e.to_string())
}

pub(crate) fn corrupt_input<E: std::fmt::Display>(e: E) -> ConvertError {
    ConvertError::CorruptInput(e.to_string())
}

pub(crate) fn dependency_missing<E: std::fmt::Display>(e: E) -> ConvertError {
    ConvertError::DependencyMissing(e.to_string())
}

pub(crate) fn ocr_fail<E: std::fmt::Display>(e: E) -> ConvertError {
    ConvertError::Ocr(e.to_string())
}

/// Typed constructors reserved for call sites with clear evidence.
/// Kept even when unused in core today so converters adopt taxonomy without
/// inventing ad-hoc `Failed` strings or parsing opaque messages.
#[allow(dead_code)]
pub(crate) fn integrity_fail<E: std::fmt::Display>(e: E) -> ConvertError {
    ConvertError::Integrity(e.to_string())
}

#[allow(dead_code)]
pub(crate) fn timeout_fail<E: std::fmt::Display>(e: E) -> ConvertError {
    ConvertError::Timeout(e.to_string())
}

#[allow(dead_code)]
pub(crate) fn resource_fail<E: std::fmt::Display>(e: E) -> ConvertError {
    ConvertError::Resource(e.to_string())
}

#[allow(dead_code)]
pub(crate) fn provider_fail<E: std::fmt::Display>(e: E) -> ConvertError {
    ConvertError::Provider(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        corrupt_input, dependency_missing, fail, integrity_fail, ocr_fail, provider_fail,
        resource_fail, timeout_fail,
    };

    #[test]
    fn typed_helpers_preserve_display_and_stable_codes() {
        assert_eq!(fail("x").code(), "failed");
        assert_eq!(corrupt_input("bad pdf").code(), "corrupt_input");
        assert_eq!(
            dependency_missing("thiếu PDFium").code(),
            "dependency_missing"
        );
        assert_eq!(ocr_fail("OCR lỗi").code(), "ocr");
        assert_eq!(integrity_fail("hash lệch").code(), "integrity");
        assert_eq!(timeout_fail("hết giờ").code(), "timeout");
        assert_eq!(resource_fail("oom").code(), "resource");
        assert_eq!(provider_fail("HTTP 503").code(), "provider");
        // Messages stay intact (no reclassification / rewriting of caller text).
        assert!(dependency_missing("thiếu PDFium")
            .to_string()
            .contains("thiếu PDFium"));
        assert!(ocr_fail("trang 3: tesseract exit 1")
            .to_string()
            .contains("trang 3: tesseract exit 1"));
    }
}
