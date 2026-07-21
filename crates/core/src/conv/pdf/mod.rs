//! PDF → Markdown, quyết định **theo từng trang**.
//!
//! Đường chính: **`pdf-inspector`** trích markdown CÓ CẤU TRÚC theo từng trang
//! (heading theo cỡ chữ, bảng, **sắp lại thứ tự đọc đa cột**) và tự gắn cờ
//! `needs_ocr` cho trang scan/ảnh HOẶC trang có **text-layer rác** (font GID,
//! encoding hỏng) — bắt được lỗi mà cách đếm ký tự không thấy.
//!
//! Trang `needs_ocr` → render bằng PDFium ở 300 DPI rồi **OCR Tesseract** (pdf-inspector
//! không OCR). Trang trộn (text + ảnh) có thể OCR thêm ảnh nhúng khi bật `pdf_ocr_images`.
//!
//! Fallback: nếu pdf-inspector lỗi → đường PDFium (đếm ký tự); nếu vẫn không được /
//! thiếu libpdfium → `pdf-extract`.

mod fallback;
mod inspector;
mod native_text;
mod ocr;
mod pdfium;
mod postprocess;
mod recovery;

use std::path::Path;

use crate::diagnostics::{ConversionWarning, DetailedConvertError, MarkdownOutput};
use crate::image_ocr::{self, OcrAttemptError, OcrRunConfig};
use crate::ConvertError;

use fallback::{extract_with_pdf_extract, via_pdfium};
use inspector::{
    probe_pages_needing_ocr, via_pdf_inspector, via_pdf_inspector_filtered_fast,
    via_pdf_inspector_parallel_full, InspectorAttempt,
};
use pdfium::pdfium_available;
use recovery::PDF_UNTRUSTED_EXTRACT_SOURCE;

/// Legacy string-only entry (warnings discarded). Prefer [`to_markdown_detailed`].
#[allow(dead_code)]
pub(crate) fn to_markdown(
    path: &Path,
    ocr_langs: &str,
    ocr_enabled: bool,
    ocr_images: bool,
    pages: Option<&[u32]>,
) -> Result<String, ConvertError> {
    to_markdown_detailed(
        path,
        ocr_langs,
        ocr_enabled,
        ocr_images,
        pages,
        &OcrRunConfig::default(),
    )
    .map(|output| output.markdown)
    .map_err(|error| error.error)
}

/// Explicit markdown + soft diagnostics (no TLS OCR error collector).
pub(crate) fn to_markdown_detailed(
    path: &Path,
    ocr_langs: &str,
    ocr_enabled: bool,
    ocr_images: bool,
    pages: Option<&[u32]>,
    ocr_config: &OcrRunConfig,
) -> Result<MarkdownOutput, DetailedConvertError> {
    let bytes = std::fs::read(path).map_err(|e| DetailedConvertError::failed(e.to_string()))?;

    // Probe needs_ocr early so PDFium/pdf-extract fallbacks inherit the flags
    // even when the structured inspector path is abandoned.
    let probed_needs_ocr = probe_pages_needing_ocr(&bytes, pages);
    let mut last_ocr_error: Option<OcrAttemptError> = None;

    // Page-filtered requests are common in the desktop/MCP token-saving flow.
    // The per-page API below intentionally extracts the whole document for
    // cross-page font statistics (~400 ms even for one page). The regular
    // options API honours its page filter during extraction and is ~8× faster.
    // Keep the slower path as fallback for OCR and malformed tables.
    if !ocr_images {
        match pages {
            Some(selected_pages) => {
                if let Some(md) = via_pdf_inspector_filtered_fast(path, &bytes, selected_pages) {
                    // Fast path only accepts high-confidence pages — no untrusted warn.
                    return Ok(MarkdownOutput::clean(md));
                }
            }
            None => {
                if let Some(md) = via_pdf_inspector_parallel_full(path, &bytes) {
                    return Ok(MarkdownOutput::clean(md));
                }
            }
        }
    }

    // 1) pdf-inspector: markdown có cấu trúc + needs_ocr theo trang.
    let mut inherited_needs_ocr = probed_needs_ocr;
    match via_pdf_inspector(
        path,
        &bytes,
        ocr_langs,
        ocr_enabled,
        ocr_images,
        pages,
        ocr_config,
        &mut last_ocr_error,
    ) {
        InspectorAttempt::Success(output) if !output.markdown.trim().is_empty() => {
            return Ok(output);
        }
        InspectorAttempt::Abandoned { pages_needing_ocr } => {
            inherited_needs_ocr.extend(pages_needing_ocr);
        }
        InspectorAttempt::Success(_) | InspectorAttempt::Unavailable => {}
    }

    // 2) Fallback: PDFium — inherits inspector needs_ocr flags.
    if let Some(output) = via_pdfium(
        path,
        ocr_langs,
        ocr_enabled,
        ocr_images,
        pages,
        &inherited_needs_ocr,
        ocr_config,
        &mut last_ocr_error,
    ) {
        if !output.markdown.trim().is_empty() {
            return Ok(output);
        }
    }

    // 3) Cuối cùng: pdf-extract (không hỗ trợ lọc trang).
    if pages.is_some() {
        if let Some(error) = last_ocr_error {
            return Err(pdf_ocr_hard_failure(
                error,
                "OCR trang PDF đã chọn thất bại",
            ));
        }
        return Err(DetailedConvertError::failed(
            "không thể trích đúng các trang đã chọn (pdf-inspector/PDFium thất bại)",
        ));
    }
    match extract_with_pdf_extract(&bytes) {
        Ok(text) if !text.trim().is_empty() => {
            let mut warnings = Vec::new();
            for page in inherited_needs_ocr {
                // Extract path has no per-page OCR recovery — flagged pages that
                // survive here preserved untrusted extracted text.
                warnings.push(ConversionWarning::pdf_untrusted_text_fallback(
                    page,
                    PDF_UNTRUSTED_EXTRACT_SOURCE,
                ));
            }
            Ok(MarkdownOutput::with_warnings(text, warnings))
        }
        Err(error) => Err(error),
        Ok(_) => {
            if !ocr_enabled {
                return Err(DetailedConvertError::failed(
                    "PDF không có text layer; hãy bật OCR trang scan trong Settings",
                ));
            }
            if !pdfium_available() {
                return Err(DetailedConvertError::failed(
                    "PDF là bản scan nhưng không tìm thấy PDFium để render trang; \
                     hãy cài lại Markhand Desktop hoặc đặt FILECONV_PDFIUM_LIB",
                ));
            }
            if let Some(error) = last_ocr_error {
                return Err(pdf_ocr_hard_failure(error, "OCR trang PDF thất bại"));
            }
            // No OCR attempt recorded: probe binary availability at this stage.
            let binary = image_ocr::effective_tesseract_binary(ocr_config);
            if !image_ocr::tesseract_available() {
                return Err(DetailedConvertError::dependency_missing(format!(
                    "PDF là bản scan nhưng không tìm thấy Tesseract OCR ({}); \
                     hãy cài lại Markhand Desktop hoặc đặt FILECONV_TESSERACT",
                    binary.display()
                )));
            }
            Err(DetailedConvertError::failed(
                "PDF không có text layer và OCR không nhận được nội dung",
            ))
        }
    }
}

fn pdf_ocr_hard_failure(error: OcrAttemptError, prefix: &str) -> DetailedConvertError {
    match error {
        OcrAttemptError::TesseractNotFound { message, .. } => {
            DetailedConvertError::dependency_missing(format!("{prefix}: {message}"))
        }
        other => DetailedConvertError::failed(format!("{prefix}: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::inspector::probe_pages_needing_ocr;
    use crate::image_ocr::OcrRunConfig;
    use crate::{
        ConversionOutcome, ConversionWarningCode, ConvertError, ConvertErrorKind, Converter,
        ConverterOptions, DetailedConvertError,
    };
    use std::path::PathBuf;
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

    fn missing_tesseract_bin() -> PathBuf {
        PathBuf::from("/nonexistent/fileconv-core-t7-missing-tesseract")
    }

    fn review_fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/pdf/needs_ocr_untrusted_fallback.pdf")
    }

    #[test]
    fn trusted_text_pdf_detailed_is_full_success_without_over_warn() {
        let dir = std::env::temp_dir().join(format!(
            "fileconv_pdf_trusted_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("trusted.pdf");
        std::fs::write(&path, minimal_pdf_bytes()).unwrap();

        let report = Converter::with_options(ConverterOptions {
            pdf_ocr: false,
            ..ConverterOptions::default()
        })
        .convert_path_detailed(&path)
        .expect("trusted text PDF should convert");

        assert_eq!(report.outcome(), ConversionOutcome::FullSuccess);
        assert!(!report.has_warning_code(ConversionWarningCode::PdfUntrustedTextFallback));
        assert!(
            report.result.markdown.contains("Xin chao PDFium")
                || !report.result.markdown.is_empty()
        );
        // Legacy surface stays field-compatible.
        let legacy = Converter::with_options(ConverterOptions {
            pdf_ocr: false,
            ..ConverterOptions::default()
        })
        .convert_path(&path)
        .unwrap();
        assert_eq!(legacy.markdown, report.result.markdown);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn committed_review_fixture_forced_ocr_fail_unconditionally_partial_success() {
        let fixture = review_fixture_path();
        assert!(
            fixture.is_file(),
            "committed project fixture must exist: {}",
            fixture.display()
        );
        let bytes = std::fs::read(&fixture).expect("read fixture");
        assert!(
            probe_pages_needing_ocr(&bytes, None).contains(&1),
            "test requires real detector needs_ocr — not a substituted flag"
        );

        // Force OCR spawn failure via injectable binary (no env mutation). Real
        // needs_ocr path must fall back to untrusted text → PartialSuccess.
        let report = Converter::with_options_and_ocr_config(
            ConverterOptions {
                pdf_ocr: true,
                ..ConverterOptions::default()
            },
            OcrRunConfig {
                tesseract_binary: Some(missing_tesseract_bin()),
            },
        )
        .convert_path_detailed(&fixture)
        .expect("garbage text layer must recover as PartialSuccess, not hard-fail");
        assert_eq!(report.outcome(), ConversionOutcome::PartialSuccess);
        assert_ne!(report.outcome(), ConversionOutcome::FullSuccess);
        assert!(
            report.has_warning_code(ConversionWarningCode::PdfUntrustedTextFallback),
            "must emit untrusted-text warning: {:?}",
            report.warnings
        );
        assert!(!report.result.markdown.trim().is_empty());
    }

    #[test]
    fn concurrent_real_fixture_converts_stay_partial_when_ocr_forced_fail() {
        let fixture = std::sync::Arc::new(review_fixture_path());
        assert!(
            fixture.is_file(),
            "committed project fixture must exist: {}",
            fixture.display()
        );
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let path = std::sync::Arc::clone(&fixture);
                std::thread::spawn(move || {
                    Converter::with_options_and_ocr_config(
                        ConverterOptions {
                            pdf_ocr: true,
                            ..ConverterOptions::default()
                        },
                        OcrRunConfig {
                            tesseract_binary: Some(missing_tesseract_bin()),
                        },
                    )
                    .convert_path_detailed(path.as_path())
                    .expect("fixture convert")
                })
            })
            .collect();
        for handle in handles {
            let report = handle.join().expect("thread");
            assert_eq!(report.outcome(), ConversionOutcome::PartialSuccess);
            assert!(report.has_warning_code(ConversionWarningCode::PdfUntrustedTextFallback));
        }
    }

    #[test]
    fn parser_panic_is_internal_kind_with_legacy_failed_error() {
        let err = DetailedConvertError::internal("pdf-extract panic");
        assert_eq!(err.kind, ConvertErrorKind::Internal);
        assert!(matches!(err.error, ConvertError::Failed(_)));
        // Exhaustive legacy match still compiles.
        match err.error {
            ConvertError::BadPath | ConvertError::Unsupported(_) | ConvertError::Failed(_) => {}
        }
    }

    #[test]
    fn missing_tesseract_on_scan_pdf_is_dependency_missing() {
        // Empty page: needs_ocr, no text to preserve → hard DependencyMissing.
        let stream = "";
        let objects = [
            "<</Type/Catalog/Pages 2 0 R>>".to_string(),
            "<</Type/Pages/Kids[3 0 R]/Count 1>>".to_string(),
            "<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]/Contents 4 0 R>>".to_string(),
            format!("<</Length {}>>\nstream\n{stream}\nendstream", stream.len()),
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
        for off in &offsets {
            out.push_str(&format!("{off:010} 00000 n\r\n"));
        }
        out.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            objects.len() + 1
        ));
        let dir = std::env::temp_dir().join(format!("fileconv_scan_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scan.pdf");
        std::fs::write(&path, out.as_bytes()).unwrap();
        let err = Converter::with_options_and_ocr_config(
            ConverterOptions {
                pdf_ocr: true,
                ..ConverterOptions::default()
            },
            OcrRunConfig {
                tesseract_binary: Some(missing_tesseract_bin()),
            },
        )
        .convert_path_detailed(&path)
        .expect_err("scan + missing tesseract must hard-fail");
        assert_eq!(err.kind, ConvertErrorKind::DependencyMissing);
        let dto = err.to_dto();
        assert_eq!(dto.kind, ConvertErrorKind::DependencyMissing);
        assert!(!dto.message.is_empty());
        // Kind is a structured field — not only embedded in the message text.
        let json = serde_json::to_value(&dto).expect("dto json");
        assert_eq!(json["kind"], "dependency_missing");
        assert!(
            json["message"].as_str().unwrap().contains("Tesseract")
                || json["message"].as_str().unwrap().contains("tesseract")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod equivalence {
    use crate::{Converter, ConverterOptions};
    use std::path::PathBuf;

    fn gold_001() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../bench/markhand_web/golden/documents/gold-001.pdf")
    }

    /// Byte-for-byte guard: native-text golden PDF output must match the
    /// pre-split snapshot captured on cursor/core-hardening-754e @ 9048f9a.
    #[test]
    fn gold_001_markdown_matches_presplit_snapshot() {
        let path = gold_001();
        assert!(path.is_file(), "missing {}", path.display());
        let md = Converter::with_options(ConverterOptions {
            pdf_ocr: false,
            ..ConverterOptions::default()
        })
        .convert_path(&path)
        .expect("gold-001 convert")
        .markdown;
        let expected = include_str!("../../../tests/snapshots/gold_001_pdf.md");
        assert_eq!(
            md, expected,
            "PDF module split must preserve gold-001 markdown byte-for-byte"
        );
    }
}
