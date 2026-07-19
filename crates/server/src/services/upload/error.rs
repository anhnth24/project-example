//! Typed upload errors, dispositions, and redacted reason codes.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use thiserror::Error;
use uuid::Uuid;

use crate::api::ApiError;

/// Terminal intake disposition (P0-09 §7 + Accepted for benign controls).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// Syntactically valid and policy-clean; stored in quarantine for conversion.
    Accepted,
    /// Stored for review; must not be indexed until a later policy promotes it.
    Quarantined,
    /// Do not store/index; return a user-safe error.
    Rejected,
}

impl Disposition {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Quarantined => "quarantined",
            Self::Rejected => "rejected",
        }
    }
}

/// Threat classification for audit (redacted; never includes filenames/content).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreatClass {
    ExtensionSpoof,
    MimeMismatch,
    UnsupportedFormat,
    ArchiveBomb,
    ArchiveTraversal,
    NestedArchive,
    MalformedOoxml,
    ParserCorruption,
    Oversize,
    TruncatedUpload,
    PdfPageBomb,
    ImagePixelBomb,
    AudioDurationLimit,
    CsvFormula,
    PromptInjection,
    ActiveContent,
    PermissionDenied,
    StorageFailure,
    MultipartInvalid,
    Internal,
}

impl ThreatClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExtensionSpoof => "extension_spoof",
            Self::MimeMismatch => "mime_mismatch",
            Self::UnsupportedFormat => "unsupported_format",
            Self::ArchiveBomb => "archive_bomb",
            Self::ArchiveTraversal => "archive_path_traversal",
            Self::NestedArchive => "nested_archive",
            Self::MalformedOoxml => "malformed_ooxml",
            Self::ParserCorruption => "parser_corruption",
            Self::Oversize => "oversize",
            Self::TruncatedUpload => "truncated_upload",
            Self::PdfPageBomb => "pdf_page_bomb",
            Self::ImagePixelBomb => "image_pixel_bomb",
            Self::AudioDurationLimit => "audio_duration_limit",
            Self::CsvFormula => "csv_formula",
            Self::PromptInjection => "prompt_injection",
            Self::ActiveContent => "active_content",
            Self::PermissionDenied => "permission_denied",
            Self::StorageFailure => "storage_failure",
            Self::MultipartInvalid => "multipart_invalid",
            Self::Internal => "internal",
        }
    }
}

/// Stable, redacted reason codes (safe for logs and API details).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasonCode {
    ExtensionMagicMismatch,
    MagicUnrecognized,
    UploadTooLarge,
    StreamInterrupted,
    ArchiveEntryLimit,
    ArchiveUncompressedLimit,
    ArchiveCompressionRatio,
    ArchivePathTraversal,
    NestedArchiveEntry,
    MissingContentTypes,
    MissingFormatPaths,
    MalformedArchive,
    MalformedXml,
    PdfMissingEof,
    PdfPageLimit,
    ImagePixelLimit,
    AudioDurationReview,
    CsvFormulaReview,
    PromptInjectionReview,
    HtmlActiveContent,
    PermissionDenied,
    StorageUnavailable,
    MultipartMissingFile,
    MultipartTooManyFiles,
    MultipartTooManyParts,
    MultipartHeaderTooLarge,
    MultipartTimeout,
    FailClosed,
}

impl ReasonCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExtensionMagicMismatch => "extension_magic_mismatch",
            Self::MagicUnrecognized => "magic_unrecognized",
            Self::UploadTooLarge => "upload_too_large",
            Self::StreamInterrupted => "stream_interrupted",
            Self::ArchiveEntryLimit => "archive_entry_limit",
            Self::ArchiveUncompressedLimit => "archive_uncompressed_limit",
            Self::ArchiveCompressionRatio => "archive_compression_ratio",
            Self::ArchivePathTraversal => "archive_path_traversal",
            Self::NestedArchiveEntry => "nested_archive_entry",
            Self::MissingContentTypes => "missing_content_types",
            Self::MissingFormatPaths => "missing_format_paths",
            Self::MalformedArchive => "malformed_archive",
            Self::MalformedXml => "malformed_xml",
            Self::PdfMissingEof => "pdf_missing_eof",
            Self::PdfPageLimit => "pdf_page_limit",
            Self::ImagePixelLimit => "image_pixel_limit",
            Self::AudioDurationReview => "audio_duration_review",
            Self::CsvFormulaReview => "csv_formula_review",
            Self::PromptInjectionReview => "prompt_injection_review",
            Self::HtmlActiveContent => "html_active_content",
            Self::PermissionDenied => "permission_denied",
            Self::StorageUnavailable => "storage_unavailable",
            Self::MultipartMissingFile => "multipart_missing_file",
            Self::MultipartTooManyFiles => "multipart_too_many_files",
            Self::MultipartTooManyParts => "multipart_too_many_parts",
            Self::MultipartHeaderTooLarge => "multipart_header_too_large",
            Self::MultipartTimeout => "multipart_timeout",
            Self::FailClosed => "fail_closed",
        }
    }
}

/// Typed upload failure (never carries raw filenames or content).
#[derive(Debug, Error)]
pub enum UploadError {
    #[error("upload rejected")]
    Rejected {
        threat: ThreatClass,
        reason: ReasonCode,
    },
    #[error("permission denied")]
    PermissionDenied,
    #[error("storage unavailable")]
    StorageUnavailable,
    #[error("multipart invalid")]
    MultipartInvalid { reason: ReasonCode },
    #[error("internal upload error")]
    Internal,
}

impl UploadError {
    pub const fn rejected(threat: ThreatClass, reason: ReasonCode) -> Self {
        Self::Rejected { threat, reason }
    }

    pub fn threat_class(&self) -> Option<ThreatClass> {
        match self {
            Self::Rejected { threat, .. } => Some(*threat),
            Self::PermissionDenied => Some(ThreatClass::PermissionDenied),
            Self::StorageUnavailable => Some(ThreatClass::StorageFailure),
            Self::MultipartInvalid { .. } => Some(ThreatClass::MultipartInvalid),
            Self::Internal => Some(ThreatClass::Internal),
        }
    }

    pub fn reason_code(&self) -> ReasonCode {
        match self {
            Self::Rejected { reason, .. } => *reason,
            Self::PermissionDenied => ReasonCode::PermissionDenied,
            Self::StorageUnavailable => ReasonCode::StorageUnavailable,
            Self::MultipartInvalid { reason } => *reason,
            Self::Internal => ReasonCode::FailClosed,
        }
    }

    pub fn status_code(&self) -> StatusCode {
        match self {
            Self::Rejected {
                threat: ThreatClass::Oversize,
                ..
            } => StatusCode::PAYLOAD_TOO_LARGE,
            Self::Rejected { .. } | Self::MultipartInvalid { .. } => StatusCode::BAD_REQUEST,
            Self::PermissionDenied => StatusCode::FORBIDDEN,
            Self::StorageUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn api_code(&self) -> &'static str {
        match self {
            Self::Rejected {
                threat: ThreatClass::Oversize,
                ..
            } => "upload_too_large",
            Self::Rejected { .. } => "upload_rejected",
            Self::PermissionDenied => "permission_denied",
            Self::StorageUnavailable => "storage_unavailable",
            Self::MultipartInvalid { .. } => "multipart_invalid",
            Self::Internal => "internal_error",
        }
    }

    pub fn user_message(&self) -> &'static str {
        match self {
            Self::Rejected {
                threat: ThreatClass::Oversize,
                ..
            } => "Upload exceeds the maximum allowed size",
            Self::Rejected { .. } => "Upload was rejected by security policy",
            Self::PermissionDenied => "Permission denied",
            Self::StorageUnavailable => "Object storage is unavailable",
            Self::MultipartInvalid { .. } => "Multipart upload is invalid",
            Self::Internal => "Upload failed",
        }
    }

    pub fn into_response_with_request_id(self, request_id: &str) -> Response {
        let details = serde_json::json!({
            "disposition": Disposition::Rejected.as_str(),
            "threatClass": self.threat_class().map(|t| t.as_str()),
            "reasonCode": self.reason_code().as_str(),
        });
        (
            self.status_code(),
            Json(ApiError {
                code: self.api_code().into(),
                message: self.user_message().into(),
                request_id: request_id.to_string(),
                details: Some(details),
            }),
        )
            .into_response()
    }
}

impl IntoResponse for UploadError {
    fn into_response(self) -> Response {
        self.into_response_with_request_id(&Uuid::new_v4().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_errors_never_embed_filenames() {
        let err = UploadError::rejected(
            ThreatClass::ExtensionSpoof,
            ReasonCode::ExtensionMagicMismatch,
        );
        let rendered = format!("{err:?}");
        assert!(!rendered.contains("passwd"));
        assert!(!rendered.contains(".pdf"));
        assert_eq!(err.api_code(), "upload_rejected");
        assert_eq!(err.status_code(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn oversize_maps_to_413() {
        let err = UploadError::rejected(ThreatClass::Oversize, ReasonCode::UploadTooLarge);
        assert_eq!(err.status_code(), StatusCode::PAYLOAD_TOO_LARGE);
    }
}
