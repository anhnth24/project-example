//! Additive conversion diagnostics (CORE-T7).
//!
//! Legacy [`crate::ConversionResult`] / [`crate::ConvertError`] stay unchanged.
//! Soft degradation and typed error kinds live here and are returned only from
//! [`crate::Converter::convert_path_detailed`].
//!
//! [`ConversionReport::new`] is the sole public construction boundary for soft
//! warnings: it deduplicates and sorts so callers never depend on thread
//! completion order when pages are recovered concurrently.

use serde::{Deserialize, Serialize};

use crate::{ConversionResult, ConvertError};

/// Serializable warning codes for structured surfaces (CLI/MCP/desktop JSON).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversionWarningCode {
    /// `needs_ocr` page kept untrusted text after OCR recovery failed.
    PdfUntrustedTextFallback,
}

impl ConversionWarningCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PdfUntrustedTextFallback => "pdf_untrusted_text_fallback",
        }
    }
}

/// Structured soft diagnostic for partial / degraded success paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversionWarning {
    pub code: ConversionWarningCode,
    /// Converter / recovery stage (stable, not localized).
    pub source: String,
    /// 1-indexed page when the warning is page-scoped.
    pub page: Option<u32>,
    /// Human-readable detail (Vietnamese UI/logs). Preserved as authored.
    pub message: String,
}

impl ConversionWarning {
    pub fn pdf_untrusted_text_fallback(page_1idx: u32, source: impl Into<String>) -> Self {
        Self {
            code: ConversionWarningCode::PdfUntrustedTextFallback,
            source: source.into(),
            page: Some(page_1idx),
            message: format!(
                "trang {page_1idx}: OCR thất bại — giữ text-layer/native không đáng tin \
                 (partial success)"
            ),
        }
    }
}

/// Outcome of a successful detailed conversion (`Err` is hard failure).
///
/// Always derived from whether [`ConversionReport::warnings`] is empty — never
/// stored independently, so it cannot disagree with the warning list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversionOutcome {
    FullSuccess,
    PartialSuccess,
}

/// Additive successful conversion report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionReport {
    pub result: ConversionResult,
    pub warnings: Vec<ConversionWarning>,
}

impl ConversionReport {
    /// Build a report with deterministic warning order and deduplication.
    ///
    /// Stable key (documented): `(page, code, source, message)` where
    /// `page = None` sorts **before** any `Some(page)` (document-scoped
    /// warnings precede page-scoped ones), then `code.as_str()`, `source`,
    /// and `message` as byte/lexicographic strings. Exact key duplicates are
    /// collapsed; semantically distinct warnings (any key field differs) are
    /// preserved. Callers must not rely on converter thread completion order.
    pub fn new(result: ConversionResult, warnings: Vec<ConversionWarning>) -> Self {
        Self {
            result,
            warnings: normalize_conversion_warnings(warnings),
        }
    }

    /// Merge another report's warnings into `self`, keeping `self.result`.
    ///
    /// Re-applies the same sort/dedup policy as [`Self::new`] so merged output
    /// stays independent of collection order.
    pub fn merge(mut self, other: Self) -> Self {
        self.warnings.extend(other.warnings);
        self.warnings = normalize_conversion_warnings(std::mem::take(&mut self.warnings));
        self
    }

    /// Derived from `warnings` — not an independently settable field.
    pub fn outcome(&self) -> ConversionOutcome {
        if self.warnings.is_empty() {
            ConversionOutcome::FullSuccess
        } else {
            ConversionOutcome::PartialSuccess
        }
    }

    pub fn is_partial_success(&self) -> bool {
        self.outcome() == ConversionOutcome::PartialSuccess
    }

    pub fn has_warning_code(&self, code: ConversionWarningCode) -> bool {
        self.warnings.iter().any(|warning| warning.code == code)
    }
}

/// Stable sort/dedup key: `None` page before `Some`, then code/source/message.
fn conversion_warning_order_key(
    warning: &ConversionWarning,
) -> (u8, u32, &'static str, &str, &str) {
    let (page_class, page) = match warning.page {
        None => (0_u8, 0_u32),
        Some(page) => (1_u8, page),
    };
    (
        page_class,
        page,
        warning.code.as_str(),
        warning.source.as_str(),
        warning.message.as_str(),
    )
}

fn normalize_conversion_warnings(mut warnings: Vec<ConversionWarning>) -> Vec<ConversionWarning> {
    warnings.sort_by(|left, right| {
        conversion_warning_order_key(left).cmp(&conversion_warning_order_key(right))
    });
    warnings.dedup_by(|left, right| {
        conversion_warning_order_key(left) == conversion_warning_order_key(right)
    });
    warnings
}

/// Additive error kind for structured surfaces.
///
/// Only assigned at exact construction stages — never by parsing opaque strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConvertErrorKind {
    BadPath,
    Unsupported,
    Failed,
    /// Exact stage: Tesseract `Command::output` returned `ErrorKind::NotFound`.
    DependencyMissing,
    /// Exact stage: `catch_unwind` observed a parser panic (e.g. pdf-extract).
    Internal,
}

impl ConvertErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BadPath => "bad_path",
            Self::Unsupported => "unsupported",
            Self::Failed => "failed",
            Self::DependencyMissing => "dependency_missing",
            Self::Internal => "internal",
        }
    }
}

/// Detailed hard failure: legacy [`ConvertError`] plus additive [`ConvertErrorKind`].
#[derive(Debug, Clone, thiserror::Error)]
#[error("{error}")]
pub struct DetailedConvertError {
    pub error: ConvertError,
    pub kind: ConvertErrorKind,
}

impl DetailedConvertError {
    pub fn from_convert(error: ConvertError) -> Self {
        let kind = match &error {
            ConvertError::BadPath => ConvertErrorKind::BadPath,
            ConvertError::Unsupported(_) => ConvertErrorKind::Unsupported,
            ConvertError::Failed(_) => ConvertErrorKind::Failed,
        };
        Self { error, kind }
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self {
            error: ConvertError::Failed(message.into()),
            kind: ConvertErrorKind::Failed,
        }
    }

    /// Only for the Tesseract spawn-`NotFound` stage.
    pub fn dependency_missing(message: impl Into<String>) -> Self {
        Self {
            error: ConvertError::Failed(message.into()),
            kind: ConvertErrorKind::DependencyMissing,
        }
    }

    /// Only for parser `catch_unwind` panic stages.
    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            error: ConvertError::Failed(message.into()),
            kind: ConvertErrorKind::Internal,
        }
    }

    pub fn unsupported(message: &'static str) -> Self {
        Self {
            error: ConvertError::Unsupported(message),
            kind: ConvertErrorKind::Unsupported,
        }
    }

    pub fn bad_path() -> Self {
        Self {
            error: ConvertError::BadPath,
            kind: ConvertErrorKind::BadPath,
        }
    }

    /// Structured DTO for detailed CLI/MCP/desktop hard-failure surfaces.
    pub fn to_dto(&self) -> DetailedErrorDto {
        DetailedErrorDto {
            message: self.error.to_string(),
            kind: self.kind,
        }
    }
}

/// Serializable hard-failure payload for detailed commands/tools (`{message, kind}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetailedErrorDto {
    pub message: String,
    pub kind: ConvertErrorKind,
}

/// Explicit markdown + warnings returned by converters (no TLS collector).
#[derive(Debug, Clone, Default)]
pub(crate) struct MarkdownOutput {
    pub markdown: String,
    pub warnings: Vec<ConversionWarning>,
}

impl MarkdownOutput {
    pub fn clean(markdown: String) -> Self {
        Self {
            markdown,
            warnings: Vec::new(),
        }
    }

    pub fn with_warnings(markdown: String, warnings: Vec<ConversionWarning>) -> Self {
        Self { markdown, warnings }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FormatKind;
    use std::sync::{Arc, Barrier};
    use std::thread;

    fn sample_result() -> ConversionResult {
        ConversionResult {
            markdown: "ok".into(),
            title: Some("t".into()),
            format: FormatKind::Pdf,
        }
    }

    fn warning(page: Option<u32>, source: &str, message: &str) -> ConversionWarning {
        ConversionWarning {
            code: ConversionWarningCode::PdfUntrustedTextFallback,
            source: source.into(),
            page,
            message: message.into(),
        }
    }

    #[test]
    fn conversion_report_sorts_none_page_before_numbered_and_dedups() {
        let shuffled = vec![
            warning(Some(2), "b", "m2"),
            warning(None, "doc", "doc-level"),
            warning(Some(1), "a", "m1"),
            warning(Some(2), "b", "m2"),          // exact duplicate
            warning(Some(1), "a", "m1-distinct"), // distinct message kept
        ];
        let report = ConversionReport::new(sample_result(), shuffled);
        let pages: Vec<_> = report.warnings.iter().map(|w| w.page).collect();
        assert_eq!(
            pages,
            vec![None, Some(1), Some(1), Some(2)],
            "None page sorts before Some; duplicates collapsed"
        );
        assert_eq!(report.warnings.len(), 4);
        assert_eq!(report.warnings[0].message, "doc-level");
        assert_eq!(report.warnings[1].message, "m1");
        assert_eq!(report.warnings[2].message, "m1-distinct");
        assert!(report.is_partial_success());
    }

    #[test]
    fn conversion_report_is_stable_across_permutations_and_threads() {
        let base = vec![
            warning(Some(3), "z", "late"),
            warning(None, "root", "scoped"),
            warning(Some(1), "a", "first"),
            warning(Some(2), "m", "mid"),
            warning(Some(1), "a", "first"), // dup
        ];
        let expected = ConversionReport::new(sample_result(), base.clone()).warnings;

        // All rotations of the input must normalize identically.
        for shift in 0..base.len() {
            let mut rotated = base.clone();
            rotated.rotate_left(shift);
            let got = ConversionReport::new(sample_result(), rotated).warnings;
            assert_eq!(got, expected, "rotation {shift}");
        }

        let barrier = Arc::new(Barrier::new(base.len()));
        let mut handles = Vec::new();
        for shift in 0..base.len() {
            let barrier = Arc::clone(&barrier);
            let mut rotated = base.clone();
            rotated.rotate_left(shift);
            let result = sample_result();
            handles.push(thread::spawn(move || {
                barrier.wait();
                ConversionReport::new(result, rotated).warnings
            }));
        }
        for handle in handles {
            assert_eq!(handle.join().expect("thread"), expected);
        }
    }

    #[test]
    fn conversion_report_merge_and_json_preserve_stable_warning_order() {
        let left = ConversionReport::new(
            sample_result(),
            vec![
                warning(Some(2), "b", "m2"),
                warning(Some(2), "b", "m2"), // exact dup collapsed on construct
            ],
        );
        let right = ConversionReport::new(
            ConversionResult {
                markdown: "other".into(),
                title: None,
                format: FormatKind::Pdf,
            },
            vec![
                warning(None, "doc", "root"),
                warning(Some(1), "a", "m1"),
                warning(Some(2), "b", "m2"), // dup across merge boundary
            ],
        );
        let merged = left.merge(right);
        assert_eq!(merged.result.markdown, "ok");
        let pages: Vec<_> = merged.warnings.iter().map(|w| w.page).collect();
        assert_eq!(pages, vec![None, Some(1), Some(2)]);
        assert_eq!(merged.warnings.len(), 3);

        let value = serde_json::to_value(&merged).expect("serialize");
        let warning_pages: Vec<_> = value["warnings"]
            .as_array()
            .expect("warnings array")
            .iter()
            .map(|entry| entry["page"].as_u64())
            .collect();
        assert_eq!(warning_pages, vec![None, Some(1), Some(2)]);
        assert_eq!(
            value["warnings"][0]["code"],
            serde_json::json!("pdf_untrusted_text_fallback")
        );

        // Round-trip JSON must keep the normalized order (not HashMap/hash order).
        let restored: ConversionReport = serde_json::from_value(value).expect("deserialize report");
        assert_eq!(restored.warnings, merged.warnings);
    }
}
