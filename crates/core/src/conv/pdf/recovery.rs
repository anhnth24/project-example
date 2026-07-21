//! `needs_ocr` page recovery classification and untrusted-text warning sources.

use crate::diagnostics::ConversionWarning;

pub(super) const PDF_UNTRUSTED_INSPECTOR_SOURCE: &str = "pdf::needs_ocr_untrusted_inspector";

pub(super) const PDF_UNTRUSTED_PDFIUM_SOURCE: &str = "pdf::needs_ocr_untrusted_pdfium";

pub(super) const PDF_UNTRUSTED_EXTRACT_SOURCE: &str = "pdf::needs_ocr_untrusted_pdf_extract";

/// Recovery choice for a pdf-inspector `needs_ocr` page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NeedsOcrRecovery {
    TrustedNative,
    OcrRendered,
    UntrustedText,
    Unresolved,
}

/// Page-level result of `needs_ocr` recovery (markdown fragment + optional warning).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NeedsOcrPageResult {
    pub(super) recovery: NeedsOcrRecovery,
    /// `None` means the page could not be recovered (abandon inspector path).
    pub(super) markdown: Option<String>,
    pub(super) warning: Option<ConversionWarning>,
}

pub(super) fn classify_needs_ocr_recovery(
    has_trustworthy_native: bool,
    ocr_ok: bool,
    has_untrusted_text: bool,
) -> NeedsOcrRecovery {
    if has_trustworthy_native {
        NeedsOcrRecovery::TrustedNative
    } else if ocr_ok {
        NeedsOcrRecovery::OcrRendered
    } else if has_untrusted_text {
        NeedsOcrRecovery::UntrustedText
    } else {
        NeedsOcrRecovery::Unresolved
    }
}

fn untrusted_text_fallback_warning(page_1idx: u32, source: &str) -> ConversionWarning {
    ConversionWarning::pdf_untrusted_text_fallback(page_1idx, source)
}

/// Decide markdown (+ optional diagnostic) for one `needs_ocr` page.
pub(super) fn recover_needs_ocr_page(
    page_1idx: u32,
    native_text: Option<&str>,
    ocr_text: Option<&str>,
    untrusted_text: &str,
    source: &str,
) -> NeedsOcrPageResult {
    let recovery = classify_needs_ocr_recovery(
        native_text.is_some(),
        ocr_text.is_some(),
        !untrusted_text.trim().is_empty(),
    );
    match recovery {
        NeedsOcrRecovery::TrustedNative => NeedsOcrPageResult {
            recovery,
            markdown: native_text.map(|text| text.trim_end().to_string()),
            warning: None,
        },
        NeedsOcrRecovery::OcrRendered => NeedsOcrPageResult {
            recovery,
            markdown: ocr_text
                .map(|text| format!("<!-- Trang {page_1idx} (OCR) -->\n\n{}", text.trim())),
            warning: None,
        },
        NeedsOcrRecovery::UntrustedText => NeedsOcrPageResult {
            recovery,
            markdown: Some(untrusted_text.trim_end().to_string()),
            warning: Some(untrusted_text_fallback_warning(page_1idx, source)),
        },
        NeedsOcrRecovery::Unresolved => NeedsOcrPageResult {
            recovery,
            markdown: None,
            warning: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{
        classify_needs_ocr_recovery, recover_needs_ocr_page, NeedsOcrPageResult, NeedsOcrRecovery,
        PDF_UNTRUSTED_EXTRACT_SOURCE, PDF_UNTRUSTED_INSPECTOR_SOURCE, PDF_UNTRUSTED_PDFIUM_SOURCE,
    };
    use crate::ConversionWarningCode;

    #[test]
    fn classifies_needs_ocr_recovery_paths() {
        assert_eq!(
            classify_needs_ocr_recovery(true, true, true),
            NeedsOcrRecovery::TrustedNative
        );
        assert_eq!(
            classify_needs_ocr_recovery(false, true, true),
            NeedsOcrRecovery::OcrRendered
        );
        assert_eq!(
            classify_needs_ocr_recovery(false, false, true),
            NeedsOcrRecovery::UntrustedText
        );
        assert_eq!(
            classify_needs_ocr_recovery(false, false, false),
            NeedsOcrRecovery::Unresolved
        );
    }

    #[test]
    fn untrusted_text_recovery_always_pairs_warning() {
        let recovered = recover_needs_ocr_page(
            4,
            None,
            None,
            "<!-- garbled inspector text-layer -->\nGID/font rác",
            PDF_UNTRUSTED_INSPECTOR_SOURCE,
        );
        assert_eq!(recovered.recovery, NeedsOcrRecovery::UntrustedText);
        let warning = recovered.warning.expect("must warn");
        assert_eq!(
            warning.code,
            ConversionWarningCode::PdfUntrustedTextFallback
        );
        assert_eq!(warning.source, PDF_UNTRUSTED_INSPECTOR_SOURCE);
        assert_eq!(warning.page, Some(4));
    }

    #[test]
    fn trusted_native_and_ocr_paths_do_not_emit_untrusted_warning() {
        let native = recover_needs_ocr_page(
            1,
            Some("native tin cậy"),
            None,
            "inspector",
            PDF_UNTRUSTED_INSPECTOR_SOURCE,
        );
        assert_eq!(
            native,
            NeedsOcrPageResult {
                recovery: NeedsOcrRecovery::TrustedNative,
                markdown: Some("native tin cậy".into()),
                warning: None,
            }
        );
        let ocr =
            recover_needs_ocr_page(2, None, Some("ocr text"), "", PDF_UNTRUSTED_PDFIUM_SOURCE);
        assert_eq!(ocr.recovery, NeedsOcrRecovery::OcrRendered);
        assert!(ocr.warning.is_none());
        let unresolved = recover_needs_ocr_page(3, None, None, "   ", PDF_UNTRUSTED_EXTRACT_SOURCE);
        assert_eq!(unresolved.recovery, NeedsOcrRecovery::Unresolved);
        assert!(unresolved.warning.is_none());
    }
}
