//! Quarantine upload intake (P1B-I01): auth → stream → hash → allowlist → disposition.
//!
//! Quota reservation is intentionally not implemented here (P1B-I02). Callers must
//! invoke [`quota_reserve_hook`] which currently logs a TODO and returns Ok.

mod archive;
mod error;
mod limits;
mod sniff;
mod stream;

pub use archive::{reject_dangerous_entry_name, validate_zip_archive, ArchiveCheck};
pub use error::{Disposition, ReasonCode, ThreatClass, UploadError};
pub use limits::{LimitsConfig, STREAM_CHUNK_BYTES};
pub use sniff::{detect_magic, resolve_canonical_format, CanonicalFormat};
pub use stream::{stream_async_read_to_tempfile, stream_to_tempfile, StreamedUpload};

use std::io::Read;
use std::path::Path;

use tokio::fs::File as TokioFile;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::auth::permissions::{require_permission, ResolveError};
use crate::storage::keys::quarantine_key;
use crate::storage::minio::{MinioClient, ObjectIdentityMeta};
use crate::storage::ObjectKey;

/// Upload policy + limits configuration section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UploadConfig {
    pub limits: LimitsConfig,
}

impl UploadConfig {
    pub const fn policy_defaults() -> Self {
        Self {
            limits: LimitsConfig::policy_defaults(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        self.limits.validate()
    }
}

impl Default for UploadConfig {
    fn default() -> Self {
        Self::policy_defaults()
    }
}

/// Successful intake result (Accepted or Quarantined). Rejected paths use [`UploadError`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadOutcome {
    pub disposition: Disposition,
    pub threat_class: Option<ThreatClass>,
    pub reason_code: Option<ReasonCode>,
    pub object_key: ObjectKey,
    pub object_id: Uuid,
    pub sha256_hex: String,
    pub size_bytes: u64,
    pub canonical_format: CanonicalFormat,
    /// Original filename retained as metadata only (never in the key).
    pub original_filename: Option<String>,
}

/// Integration hook for P1B-I02 quota reservation (not implemented).
///
/// Callers must invoke this before streaming bytes. Today it is a no-op success
/// so intake remains fail-open on quota until I02 lands — the hook exists so the
/// call site is unambiguous.
pub fn quota_reserve_hook(_org: &OrgContext, _idempotency_key: &str, _bytes: Option<u64>) {
    // TODO(P1B-I02): atomically reserve tenant quota with an idempotency key
    // before accepting upload bytes; release on rejection/timeout.
}

/// Validate a fully-received tempfile and optionally persist to quarantine storage.
pub async fn validate_and_quarantine(
    org: &OrgContext,
    storage: &MinioClient,
    limits: &LimitsConfig,
    streamed: StreamedUpload,
    declared_filename: Option<&str>,
) -> Result<UploadOutcome, UploadError> {
    require_permission(org, "doc.upload").map_err(|error| match error {
        ResolveError::PermissionDenied => UploadError::PermissionDenied,
        _ => UploadError::Internal,
    })?;

    // TODO(P1B-I02): pass expected size into quota_reserve_hook once implemented.
    quota_reserve_hook(org, "upload", Some(streamed.size_bytes));

    let path = streamed.tempfile.path();
    let validation = match validate_bytes(path, &streamed.head, declared_filename, limits) {
        Ok(result) => result,
        Err(error) => {
            // Rejected: never leave a quarantine object.
            return Err(error);
        }
    };

    let object_id = Uuid::new_v4();
    // Filename is metadata only — quarantine_key ignores it for the path.
    let key = quarantine_key(org.org_id(), object_id, declared_filename)
        .map_err(|_| UploadError::Internal)?;

    // Defense in depth: key must never contain the client filename.
    if let Some(name) = declared_filename {
        let key_str = key.as_str();
        if !name.is_empty() && key_str.contains(name) {
            return Err(UploadError::Internal);
        }
    }

    let safe_filename = declared_filename.map(sanitize_filename_metadata);
    let meta = ObjectIdentityMeta {
        org_id: org.org_id(),
        collection_id: None,
        document_id: None,
        version_id: None,
        original_filename: safe_filename.clone(),
        canonical_format: Some(validation.format.as_str().to_string()),
        content_sha256: Some(streamed.sha256_hex.clone()),
        content_length: Some(streamed.size_bytes),
        disposition: Some(validation.disposition.as_str().to_string()),
    };

    let file = TokioFile::open(path)
        .await
        .map_err(|_| UploadError::Internal)?;
    storage
        .put_object_stream(
            org.org_id(),
            &key,
            file,
            &meta,
            content_type_for(validation.format),
        )
        .await
        .map_err(|_| UploadError::StorageUnavailable)?;

    Ok(UploadOutcome {
        disposition: validation.disposition,
        threat_class: validation.threat_class,
        reason_code: validation.reason_code,
        object_key: key,
        object_id,
        sha256_hex: streamed.sha256_hex,
        size_bytes: streamed.size_bytes,
        canonical_format: validation.format,
        original_filename: safe_filename,
    })
}

/// Stream a multipart field to tempfile then validate + quarantine.
pub async fn intake_field<R>(
    org: &OrgContext,
    storage: &MinioClient,
    limits: &LimitsConfig,
    reader: R,
    declared_filename: Option<&str>,
) -> Result<UploadOutcome, UploadError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let streamed = stream_async_read_to_tempfile(reader, limits).await?;
    validate_and_quarantine(org, storage, limits, streamed, declared_filename).await
}

#[derive(Debug)]
struct ValidationResult {
    format: CanonicalFormat,
    disposition: Disposition,
    threat_class: Option<ThreatClass>,
    reason_code: Option<ReasonCode>,
}

fn validate_bytes(
    path: &Path,
    head: &[u8],
    declared_filename: Option<&str>,
    limits: &LimitsConfig,
) -> Result<ValidationResult, UploadError> {
    let mut format = resolve_canonical_format(head, declared_filename)?;

    if format.is_zip_container() || head.starts_with(b"PK") {
        let check = validate_zip_archive(path, format, limits)?;
        format = check.format;
        // Re-check extension consistency against refined format.
        if let Some(name) = declared_filename {
            let ext = name
                .rsplit(['/', '\\'])
                .next()
                .and_then(|base| base.rsplit_once('.'))
                .map(|(_, ext)| ext.to_ascii_lowercase())
                .unwrap_or_default();
            let ok = match format {
                CanonicalFormat::Docx => ext == "docx",
                CanonicalFormat::Pptx => ext == "pptx",
                CanonicalFormat::Xlsx => ext == "xlsx",
                CanonicalFormat::Ods => ext == "ods",
                _ => true,
            };
            if !ok {
                return Err(UploadError::rejected(
                    ThreatClass::ExtensionSpoof,
                    ReasonCode::ExtensionMagicMismatch,
                ));
            }
        }
    }

    match format {
        CanonicalFormat::Pdf => preflight_pdf(path, limits)?,
        CanonicalFormat::Png
        | CanonicalFormat::Jpeg
        | CanonicalFormat::Webp
        | CanonicalFormat::Tiff
        | CanonicalFormat::Bmp => preflight_image(path, format, limits)?,
        CanonicalFormat::Wav
        | CanonicalFormat::Mp3
        | CanonicalFormat::Ogg
        | CanonicalFormat::Flac
        | CanonicalFormat::M4a => {
            if let Some(review) = preflight_audio(path, format, limits)? {
                return Ok(review);
            }
        }
        CanonicalFormat::Csv => {
            if let Some(review) = preflight_csv(path)? {
                return Ok(review);
            }
        }
        CanonicalFormat::Html => {
            if let Some(outcome) = preflight_html(path)? {
                return Ok(outcome);
            }
        }
        CanonicalFormat::PlainText
        | CanonicalFormat::Docx
        | CanonicalFormat::Pptx
        | CanonicalFormat::Xlsx
        | CanonicalFormat::Ods
        | CanonicalFormat::Xls
        | CanonicalFormat::Xlsb
        | CanonicalFormat::ZipContainer => {}
    }

    Ok(ValidationResult {
        format,
        disposition: Disposition::Accepted,
        threat_class: None,
        reason_code: None,
    })
}

fn preflight_pdf(path: &Path, limits: &LimitsConfig) -> Result<(), UploadError> {
    let mut file = std::fs::File::open(path).map_err(|_| {
        UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
    })?;
    let mut buf = Vec::new();
    // Cap PDF preflight read at upload limit (already enforced) but stream in chunks.
    file.read_to_end(&mut buf).map_err(|_| {
        UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
    })?;
    if !buf.windows(5).any(|w| w == b"%%EOF") && !buf.ends_with(b"%%EOF\n") {
        // Allow %%EOF with trailing whitespace.
        let trimmed = trim_ascii_end(&buf);
        if !trimmed.ends_with(b"%%EOF") {
            return Err(UploadError::rejected(
                ThreatClass::ParserCorruption,
                ReasonCode::PdfMissingEof,
            ));
        }
    }
    // Cheap page-count heuristic (not a full PDF parser).
    let pages = count_pdf_pages_heuristic(&buf);
    if pages > limits.max_pdf_pages {
        return Err(UploadError::rejected(
            ThreatClass::PdfPageBomb,
            ReasonCode::PdfPageLimit,
        ));
    }
    Ok(())
}

fn count_pdf_pages_heuristic(data: &[u8]) -> u32 {
    // Count `/Type /Page` not followed by `s` (Page vs Pages).
    let mut count = 0_u32;
    let mut i = 0;
    while i + 10 < data.len() {
        if &data[i..i + 5] == b"/Type" {
            let mut j = i + 5;
            while j < data.len() && data[j].is_ascii_whitespace() {
                j += 1;
            }
            if data[j..].starts_with(b"/Page") {
                let after = j + 5;
                if after >= data.len() || data[after] != b's' {
                    count = count.saturating_add(1);
                }
            }
        }
        i += 1;
    }
    count
}

fn preflight_image(
    path: &Path,
    format: CanonicalFormat,
    limits: &LimitsConfig,
) -> Result<(), UploadError> {
    let mut file = std::fs::File::open(path).map_err(|_| {
        UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
    })?;
    let mut head = [0_u8; 64];
    let n = file.read(&mut head).map_err(|_| {
        UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
    })?;
    let pixels = image_pixel_count(&head[..n], format).unwrap_or(0);
    if pixels > limits.max_image_pixels {
        return Err(UploadError::rejected(
            ThreatClass::ImagePixelBomb,
            ReasonCode::ImagePixelLimit,
        ));
    }
    Ok(())
}

fn image_pixel_count(head: &[u8], format: CanonicalFormat) -> Option<u64> {
    match format {
        CanonicalFormat::Png if head.len() >= 24 => {
            let w = u32::from_be_bytes(head[16..20].try_into().ok()?);
            let h = u32::from_be_bytes(head[20..24].try_into().ok()?);
            Some(u64::from(w).saturating_mul(u64::from(h)))
        }
        CanonicalFormat::Bmp if head.len() >= 26 => {
            let w = i32::from_le_bytes(head[18..22].try_into().ok()?).unsigned_abs();
            let h = i32::from_le_bytes(head[22..26].try_into().ok()?).unsigned_abs();
            Some(u64::from(w).saturating_mul(u64::from(h)))
        }
        // JPEG/WebP/TIFF: defer deep decode to sandbox; header presence is enough here.
        _ => Some(0),
    }
}

fn preflight_audio(
    path: &Path,
    format: CanonicalFormat,
    limits: &LimitsConfig,
) -> Result<Option<ValidationResult>, UploadError> {
    if format != CanonicalFormat::Wav {
        return Ok(None);
    }
    let mut file = std::fs::File::open(path).map_err(|_| {
        UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
    })?;
    let mut head = [0_u8; 44];
    file.read_exact(&mut head).map_err(|_| {
        UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
    })?;
    if &head[0..4] != b"RIFF" || &head[8..12] != b"WAVE" {
        return Err(UploadError::rejected(
            ThreatClass::MimeMismatch,
            ReasonCode::ExtensionMagicMismatch,
        ));
    }
    let byte_rate = u32::from_le_bytes(head[28..32].try_into().unwrap_or([0; 4]));
    let meta = file.metadata().map_err(|_| UploadError::Internal)?;
    let data_bytes = meta.len().saturating_sub(44);
    if byte_rate == 0 {
        return Ok(None);
    }
    let duration_secs = data_bytes / u64::from(byte_rate);
    if duration_secs > limits.max_audio_duration_secs {
        return Ok(Some(ValidationResult {
            format,
            disposition: Disposition::Quarantined,
            threat_class: Some(ThreatClass::AudioDurationLimit),
            reason_code: Some(ReasonCode::AudioDurationReview),
        }));
    }
    Ok(None)
}

fn preflight_csv(path: &Path) -> Result<Option<ValidationResult>, UploadError> {
    let mut file = std::fs::File::open(path).map_err(|_| UploadError::Internal)?;
    let mut buf = String::new();
    // Bound CSV preflight text read.
    let mut limited = (&mut file).take(1024 * 1024);
    limited.read_to_string(&mut buf).map_err(|_| {
        UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
    })?;
    for line in buf.lines().skip(1) {
        for cell in line.split([',', '\t']) {
            let trimmed = cell.trim().trim_matches('"');
            if trimmed.starts_with(['=', '+', '-', '@']) {
                return Ok(Some(ValidationResult {
                    format: CanonicalFormat::Csv,
                    disposition: Disposition::Quarantined,
                    threat_class: Some(ThreatClass::CsvFormula),
                    reason_code: Some(ReasonCode::CsvFormulaReview),
                }));
            }
        }
    }
    Ok(None)
}

fn preflight_html(path: &Path) -> Result<Option<ValidationResult>, UploadError> {
    let mut file = std::fs::File::open(path).map_err(|_| UploadError::Internal)?;
    let mut buf = String::new();
    let mut limited = (&mut file).take(1024 * 1024);
    limited.read_to_string(&mut buf).map_err(|_| {
        UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
    })?;
    let lower = buf.to_ascii_lowercase();
    if lower.contains("<script")
        || lower.contains("javascript:")
        || lower.contains("onerror=")
        || lower.contains("onload=")
    {
        return Err(UploadError::rejected(
            ThreatClass::ActiveContent,
            ReasonCode::HtmlActiveContent,
        ));
    }
    if lower.contains("ignore previous")
        || lower.contains("system prompt")
        || lower.contains("bỏ qua")
        || lower.contains("bo qua")
        || lower.contains("tiết lộ")
        || lower.contains("tiet lo")
    {
        return Ok(Some(ValidationResult {
            format: CanonicalFormat::Html,
            disposition: Disposition::Quarantined,
            threat_class: Some(ThreatClass::PromptInjection),
            reason_code: Some(ReasonCode::PromptInjectionReview),
        }));
    }
    Ok(None)
}

fn sanitize_filename_metadata(name: &str) -> String {
    name.chars()
        .filter(|ch| !ch.is_control())
        .take(255)
        .collect()
}

fn content_type_for(format: CanonicalFormat) -> &'static str {
    match format {
        CanonicalFormat::Pdf => "application/pdf",
        CanonicalFormat::Docx => {
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        }
        CanonicalFormat::Pptx => {
            "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        }
        CanonicalFormat::Xlsx => {
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        }
        CanonicalFormat::Ods => "application/vnd.oasis.opendocument.spreadsheet",
        CanonicalFormat::Csv => "text/csv",
        CanonicalFormat::Html => "text/html",
        CanonicalFormat::PlainText => "text/plain",
        CanonicalFormat::Png => "image/png",
        CanonicalFormat::Jpeg => "image/jpeg",
        CanonicalFormat::Webp => "image/webp",
        CanonicalFormat::Tiff => "image/tiff",
        CanonicalFormat::Bmp => "image/bmp",
        CanonicalFormat::Wav => "audio/wav",
        CanonicalFormat::Mp3 => "audio/mpeg",
        CanonicalFormat::Ogg => "audio/ogg",
        CanonicalFormat::Flac => "audio/flac",
        CanonicalFormat::M4a => "audio/mp4",
        CanonicalFormat::Xls => "application/vnd.ms-excel",
        CanonicalFormat::Xlsb => "application/vnd.ms-excel.sheet.binary.macroEnabled.12",
        CanonicalFormat::ZipContainer => "application/zip",
    }
}

fn trim_ascii_end(buf: &[u8]) -> &[u8] {
    let mut end = buf.len();
    while end > 0 && buf[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    &buf[..end]
}

/// Property-style helper: disposition is always one of the typed outcomes.
pub fn assert_disposition_is_typed(disposition: Disposition) {
    match disposition {
        Disposition::Accepted | Disposition::Quarantined | Disposition::Rejected => {}
    }
}

/// Validate streamed bytes without persisting (unit / adversarial harness).
pub fn validate_streamed_bytes(
    streamed: &StreamedUpload,
    declared_filename: Option<&str>,
    limits: &LimitsConfig,
) -> Result<
    (
        CanonicalFormat,
        Disposition,
        Option<ThreatClass>,
        Option<ReasonCode>,
    ),
    UploadError,
> {
    let result = validate_bytes(
        streamed.tempfile.path(),
        &streamed.head,
        declared_filename,
        limits,
    )?;
    Ok((
        result.format,
        result.disposition,
        result.threat_class,
        result.reason_code,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::upload::stream::stream_to_tempfile;
    use bytes::Bytes;
    use futures::stream;

    #[tokio::test]
    async fn spoof_pdf_rejects() {
        let limits = LimitsConfig::policy_defaults();
        let streamed = stream_to_tempfile(
            stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from_static(
                b"not a pdf",
            ))]),
            &limits,
        )
        .await
        .unwrap();
        let err = validate_bytes(
            streamed.tempfile.path(),
            &streamed.head,
            Some("plain-text.pdf"),
            &limits,
        )
        .unwrap_err();
        assert_eq!(err.threat_class(), Some(ThreatClass::ExtensionSpoof));
    }

    #[test]
    fn filename_sanitizer_strips_controls() {
        let safe = sanitize_filename_metadata("a\nb\0c.pdf");
        assert!(!safe.contains('\n'));
        assert!(!safe.contains('\0'));
    }

    #[test]
    fn dispositions_are_typed() {
        for d in [
            Disposition::Accepted,
            Disposition::Quarantined,
            Disposition::Rejected,
        ] {
            assert_disposition_is_typed(d);
        }
    }
}
