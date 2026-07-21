//! PDFium character-count fallback path and final pdf-extract panic containment.

use std::collections::HashSet;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use crate::diagnostics::{DetailedConvertError, MarkdownOutput};
use crate::image_ocr::{OcrAttemptError, OcrRunConfig};

use super::native_text::{native_text_is_high_confidence, native_text_is_trustworthy};
use super::ocr::{ocr_full_page, ocr_page_images, PageOcr, PAGE_TEXT_MIN_CHARS};
use super::pdfium::{pdfium_call_guard, with_pdfium};
use super::recovery::{recover_needs_ocr_page, NeedsOcrPageResult, PDF_UNTRUSTED_PDFIUM_SOURCE};

/// Đường fallback cũ: PDFium đếm ký tự để quyết text vs OCR.
///
/// `pages_needing_ocr` is inherited from pdf-inspector. Flagged pages that keep
/// native text because OCR failed emit a typed partial-success warning. Trusted
/// (unflagged) native text is not over-warned.
pub(super) fn via_pdfium(
    path: &Path,
    ocr_langs: &str,
    ocr_enabled: bool,
    ocr_images: bool,
    pages: Option<&[u32]>,
    pages_needing_ocr: &HashSet<u32>,
    ocr_config: &OcrRunConfig,
    last_ocr_error: &mut Option<OcrAttemptError>,
) -> Option<MarkdownOutput> {
    let _pdfium_guard = pdfium_call_guard();
    with_pdfium(|opt| -> Option<MarkdownOutput> {
        let pdfium = opt?;
        let doc = pdfium.load_pdf_from_file(path, None).ok()?;
        let mut out = String::new();
        let mut warnings = Vec::new();
        let mut unresolved_pages = Vec::new();
        for (i, page) in doc.pages().iter().enumerate() {
            let page_1idx = i as u32 + 1;
            // Lọc trang (1-indexed) nếu người dùng chỉ định.
            if let Some(ps) = pages {
                if !ps.contains(&page_1idx) {
                    continue;
                }
            }
            let text = page.text().map(|t| t.all()).unwrap_or_default();
            let nonspace = text.chars().filter(|c| !c.is_whitespace()).count();
            let flagged = pages_needing_ocr.contains(&page_1idx);

            if flagged {
                let ocr_text = ocr_enabled
                    .then(|| match ocr_full_page(&page, ocr_langs, ocr_config) {
                        Ok(PageOcr::Text(ocr)) => Some(ocr),
                        Ok(PageOcr::Blank) => Some(String::new()),
                        Err(error) => {
                            *last_ocr_error = Some(error);
                            None
                        }
                    })
                    .flatten();
                let trustworthy = (native_text_is_trustworthy(&text)
                    && native_text_is_high_confidence(&text))
                .then_some(text.as_str());
                let untrusted = if nonspace > 0 { text.as_str() } else { "" };
                let recovered = recover_needs_ocr_page(
                    page_1idx,
                    trustworthy,
                    ocr_text.as_deref(),
                    untrusted,
                    PDF_UNTRUSTED_PDFIUM_SOURCE,
                );
                match recovered {
                    NeedsOcrPageResult {
                        markdown: Some(page_md),
                        warning,
                        ..
                    } => {
                        out.push_str(page_md.trim_end());
                        out.push_str("\n\n");
                        if let Some(warning) = warning {
                            warnings.push(warning);
                        }
                    }
                    NeedsOcrPageResult { markdown: None, .. } => {
                        unresolved_pages.push(page_1idx);
                    }
                }
            } else if nonspace >= PAGE_TEXT_MIN_CHARS {
                out.push_str(text.trim_end());
                out.push_str("\n\n");
                if ocr_enabled && ocr_images {
                    if let Some(extra) =
                        ocr_page_images(&doc, &page, ocr_langs, i + 1, ocr_config, last_ocr_error)
                    {
                        out.push_str(&extra);
                    }
                }
            } else if ocr_enabled {
                match ocr_full_page(&page, ocr_langs, ocr_config) {
                    Ok(PageOcr::Text(ocr)) => {
                        let ocr = ocr.trim();
                        out.push_str(&format!("<!-- Trang {page_1idx} (OCR) -->\n\n"));
                        out.push_str(ocr);
                        out.push_str("\n\n");
                    }
                    Ok(PageOcr::Blank) => {}
                    Err(error) => {
                        unresolved_pages.push(page_1idx);
                        *last_ocr_error = Some(error);
                    }
                }
            } else {
                unresolved_pages.push(page_1idx);
            }
        }
        if !unresolved_pages.is_empty() || out.trim().is_empty() {
            None
        } else {
            Some(MarkdownOutput::with_warnings(out, warnings))
        }
    })
}

/// Fallback cuối: pdf-extract (có thể panic → bắt bằng catch_unwind).
pub(super) fn extract_with_pdf_extract(bytes: &[u8]) -> Result<String, DetailedConvertError> {
    let result = catch_unwind(AssertUnwindSafe(|| {
        pdf_extract::extract_text_from_mem(bytes)
    }));
    match result {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(e)) => Err(DetailedConvertError::failed(e.to_string())),
        // Exact stage: parser panic → Internal (legacy error remains Failed).
        Err(_) => Err(DetailedConvertError::internal(
            "pdf-extract panic (PDF phức tạp/không chuẩn)",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::via_pdfium;
    use crate::conv::pdf::recovery::PDF_UNTRUSTED_PDFIUM_SOURCE;
    use crate::diagnostics::MarkdownOutput;
    use crate::image_ocr::OcrRunConfig;
    use crate::ConversionWarningCode;
    use std::collections::HashSet;
    /// PDF một trang tối giản, tự tính offset xref để PDFium load được thật.
    fn minimal_pdf_bytes() -> Vec<u8> {
        let stream = "BT /F1 24 Tf 72 720 Td (Xin chao PDFium) Tj ET";
        let objects = [
            "<</Type/Catalog/Pages 2 0 R>>".to_string(),
            "<</Type/Pages/Kids[3 0 R]/Count 1>>".to_string(),
            "<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]/Contents 4 0 R\
             /Resources<</Font<</F1 5 0 R>>>>>>"
                .to_string(),
            format!("<</Length {}>>\nstream\n{stream}\nendstream", stream.len()),
            "<</Type/Font/Subtype/Type1/BaseFont/Helvetica>>".to_string(),
        ];
        let mut out = String::from("%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&format!("{} 0 obj\n{body}\nendobj\n", i + 1));
        }
        let xref_at = out.len();
        out.push_str(&format!(
            "xref\n0 {}\n0000000000 65535 f\r\n",
            objects.len() + 1
        ));
        for off in offsets {
            out.push_str(&format!("{off:010} 00000 n\r\n"));
        }
        out.push_str(&format!(
            "trailer\n<</Size {}/Root 1 0 R>>\nstartxref\n{xref_at}\n%%EOF\n",
            objects.len() + 1
        ));
        out.into_bytes()
    }

    fn has_untrusted_warning(output: &MarkdownOutput) -> bool {
        output
            .warnings
            .iter()
            .any(|w| w.code == ConversionWarningCode::PdfUntrustedTextFallback)
    }

    #[test]
    fn pdfium_fallback_warns_when_inherited_needs_ocr_keeps_untrusted_native() {
        let dir = std::env::temp_dir().join(format!(
            "fileconv_pdf_flagged_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("flagged.pdf");
        std::fs::write(&path, minimal_pdf_bytes()).unwrap();
        let cfg = OcrRunConfig::default();
        let mut last = None;

        // Inherit needs_ocr=1 with OCR disabled → preserve native text + warn.
        let flagged = HashSet::from([1u32]);
        let output = via_pdfium(&path, "eng", false, false, None, &flagged, &cfg, &mut last)
            .expect("PDFium should return native text for minimal PDF");
        assert!(
            output.warnings.iter().any(|w| w.code
                == ConversionWarningCode::PdfUntrustedTextFallback
                && w.page == Some(1)
                && w.source == PDF_UNTRUSTED_PDFIUM_SOURCE),
            "flagged page keeping native without OCR must warn: {:?}",
            output.warnings
        );
        assert!(!output.markdown.trim().is_empty());

        // Unflagged trusted fallback must not over-warn.
        let mut last = None;
        let clean = via_pdfium(
            &path,
            "eng",
            false,
            false,
            None,
            &HashSet::new(),
            &cfg,
            &mut last,
        )
        .expect("unflagged path");
        assert!(
            clean.warnings.is_empty(),
            "trusted PDFium text must not emit partial warning"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_pdfium_flagged_paths_do_not_cross_contaminate_warnings() {
        let dir = std::env::temp_dir().join(format!(
            "fileconv_pdf_conc_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.pdf");
        std::fs::write(&path, minimal_pdf_bytes()).unwrap();
        let path = std::sync::Arc::new(path);
        let handles: Vec<_> = (0..6)
            .map(|i| {
                let path = std::sync::Arc::clone(&path);
                std::thread::spawn(move || {
                    let flags = if i % 2 == 0 {
                        HashSet::from([1u32])
                    } else {
                        HashSet::new()
                    };
                    let cfg = OcrRunConfig::default();
                    let mut last = None;
                    via_pdfium(&path, "eng", false, false, None, &flags, &cfg, &mut last)
                })
            })
            .collect();
        for (i, handle) in handles.into_iter().enumerate() {
            let output = handle.join().unwrap().expect("pdfium output");
            if i % 2 == 0 {
                assert!(has_untrusted_warning(&output));
            } else {
                assert!(
                    output.warnings.is_empty(),
                    "unflagged concurrent convert leaked warnings: {:?}",
                    output.warnings
                );
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
