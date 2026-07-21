//! Additive conversion diagnostics (CORE-T7).
//!
//! Legacy [`crate::ConversionResult`] / [`crate::ConvertError`] stay unchanged.
//! Soft degradation and typed error kinds live here and are returned only from
//! [`crate::Converter::convert_path_detailed`].

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
    pub fn new(result: ConversionResult, warnings: Vec<ConversionWarning>) -> Self {
        Self { result, warnings }
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
