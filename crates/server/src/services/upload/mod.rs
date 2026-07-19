//! Quarantine upload intake (P1B-I01): auth → stream → hash → allowlist → disposition.
//!
//! Upload routes must call [`quota_reserve_hook`] after streaming has produced a
//! server-measured byte count, then finalize the returned reservation only after
//! object-store persistence succeeds. Failures after reservation must refund; if
//! a process crashes, the reservation TTL releases admission and the expiry sweep
//! marks it terminal.

mod archive;
mod error;
mod limits;
mod sniff;
mod stream;

pub use archive::{reject_dangerous_entry_name, validate_zip_archive, ArchiveCheck};
pub use error::{Disposition, ReasonCode, ThreatClass, UploadError};
pub use limits::{LimitsConfig, STREAM_CHUNK_BYTES};
pub use sniff::{
    declared_extension, detect_magic, extension_matches, mime_matches, resolve_canonical_format,
    CanonicalFormat,
};
pub use stream::{
    stream_async_read_to_tempfile, stream_to_tempfile, stream_to_tempfile_with_idle_timeout,
    StreamedUpload,
};

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};

use tokio::fs::File as TokioFile;
use tokio::io::{AsyncSeekExt, SeekFrom as TokioSeekFrom};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::auth::permissions::{require_permission, ResolveError};
use crate::services::quota::{
    self, QuotaError, QuotaSnapshot, UploadQuotaReservation, DEFAULT_RESERVATION_TTL,
};
use crate::storage::keys::quarantine_key;
use crate::storage::minio::{MinioClient, ObjectIdentityMeta, ObjectPutVerification};
use crate::storage::ObjectKey;

use self::archive::validate_zip_reader;

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

/// Integration hook for P1B-I02 quota reservation.
///
/// The amount comes from `StreamedUpload::size_bytes`, not from a request field.
/// One upload reserves storage bytes and one document slot under server-derived
/// child keys. `reservation_key` must be server-minted by the caller.
pub async fn quota_reserve_hook(
    pool: &deadpool_postgres::Pool,
    org: &OrgContext,
    reservation_key: &str,
    bytes: u64,
) -> Result<UploadQuotaReservation, QuotaError> {
    quota::reserve_upload(pool, org, reservation_key, bytes, DEFAULT_RESERVATION_TTL).await
}

#[derive(Debug)]
pub struct QuotaSettledUpload {
    pub outcome: UploadOutcome,
    pub quota_snapshot: QuotaSnapshot,
}

#[derive(Debug)]
pub enum QuotaSettledUploadError {
    Upload(UploadError),
    Quota {
        error: QuotaError,
        outcome: Box<UploadOutcome>,
    },
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_quota_settled_quarantine(
    pool: deadpool_postgres::Pool,
    org: OrgContext,
    storage: MinioClient,
    limits: LimitsConfig,
    streamed: StreamedUpload,
    declared_filename: Option<String>,
    declared_content_type: Option<String>,
    reservation_key: String,
) -> tokio::task::JoinHandle<Result<QuotaSettledUpload, QuotaSettledUploadError>> {
    tokio::spawn(async move {
        let outcome = validate_and_quarantine(
            &org,
            &storage,
            &limits,
            streamed,
            declared_filename.as_deref(),
            declared_content_type.as_deref(),
        )
        .await;
        match outcome {
            Ok(outcome) => match quota::finalize_upload(&pool, &org, &reservation_key).await {
                Ok(settlement) => Ok(QuotaSettledUpload {
                    outcome,
                    quota_snapshot: settlement.storage_quota,
                }),
                Err(error) => {
                    // In-process settlement guarantee: after storage succeeds but quota
                    // finalize fails, release quota before deleting the untrusted
                    // quarantine object. If either best-effort cleanup step fails, the
                    // reservation TTL/sweep prevents permanent over-limit state and the
                    // quarantine object remains eligible for later GC.
                    // TODO(I03/I07): durable finalize/cleanup reconciliation on crash / double-failure.
                    if let Err(refund_error) =
                        quota::refund_upload(&pool, &org, &reservation_key).await
                    {
                        eprintln!(
                            "fileconv-server: quota refund after finalize failure failed; \
                             reservation_key={} code={}",
                            reservation_key,
                            refund_error.code()
                        );
                    }
                    if let Err(cleanup_error) = storage
                        .cleanup_generated_object(org.org_id(), &outcome.object_key)
                        .await
                    {
                        eprintln!(
                            "fileconv-server: quota finalize failed and upload cleanup failed; \
                             reservation_key={} code={}",
                            reservation_key,
                            cleanup_error.code()
                        );
                    }
                    Err(QuotaSettledUploadError::Quota {
                        error,
                        outcome: Box::new(outcome),
                    })
                }
            },
            Err(error) => {
                if let Err(refund_error) = quota::refund_upload(&pool, &org, &reservation_key).await
                {
                    eprintln!(
                        "fileconv-server: quota refund after upload failure failed: {}",
                        refund_error.code()
                    );
                }
                Err(QuotaSettledUploadError::Upload(error))
            }
        }
    })
}

/// Validate a fully-received tempfile and optionally persist to quarantine storage.
pub async fn validate_and_quarantine(
    org: &OrgContext,
    storage: &MinioClient,
    limits: &LimitsConfig,
    streamed: StreamedUpload,
    declared_filename: Option<&str>,
    declared_content_type: Option<&str>,
) -> Result<UploadOutcome, UploadError> {
    require_permission(org, "doc.upload").map_err(|error| match error {
        ResolveError::PermissionDenied => UploadError::PermissionDenied,
        _ => UploadError::Internal,
    })?;

    let mut validation_file = streamed.rewinded_file_clone()?;
    let head = streamed.head.clone();
    let declared_filename_owned = declared_filename.map(str::to_owned);
    let declared_content_type_owned = declared_content_type.map(str::to_owned);
    let limits_for_validation = *limits;
    let validation = tokio::task::spawn_blocking(move || {
        validate_file(
            &mut validation_file,
            &head,
            declared_filename_owned.as_deref(),
            declared_content_type_owned.as_deref(),
            &limits_for_validation,
        )
    })
    .await
    .map_err(|_| UploadError::Internal)??;

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

    let mut file = TokioFile::from_std(streamed.rewinded_file_clone()?);
    file.seek(TokioSeekFrom::Start(0))
        .await
        .map_err(|_| UploadError::Internal)?;
    storage
        .put_object_stream(
            org.org_id(),
            &key,
            file,
            &meta,
            content_type_for(validation.format),
            ObjectPutVerification {
                expected_len: streamed.size_bytes,
                expected_sha256: &streamed.sha256_hex,
            },
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
    declared_content_type: Option<&str>,
) -> Result<UploadOutcome, UploadError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let streamed = stream_async_read_to_tempfile(reader, limits).await?;
    validate_and_quarantine(
        org,
        storage,
        limits,
        streamed,
        declared_filename,
        declared_content_type,
    )
    .await
}

#[derive(Debug)]
struct ValidationResult {
    format: CanonicalFormat,
    disposition: Disposition,
    threat_class: Option<ThreatClass>,
    reason_code: Option<ReasonCode>,
}

fn validate_file(
    file: &mut File,
    head: &[u8],
    declared_filename: Option<&str>,
    declared_content_type: Option<&str>,
    limits: &LimitsConfig,
) -> Result<ValidationResult, UploadError> {
    let mut format = resolve_canonical_format(head, declared_filename)?;

    if format.is_zip_container() || head.starts_with(b"PK") {
        file.seek(SeekFrom::Start(0)).map_err(|_| {
            UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
        })?;
        let check = validate_zip_reader(&mut *file, format, limits)?;
        format = check.format;
        // Re-check extension consistency against refined format.
        if let Some(name) = declared_filename {
            if declared_extension(name).is_none_or(|ext| !extension_matches(format, ext)) {
                return Err(UploadError::rejected(
                    ThreatClass::ExtensionSpoof,
                    ReasonCode::ExtensionMagicMismatch,
                ));
            }
        }
    }
    if let Some(content_type) = declared_content_type {
        if !mime_matches(format, content_type) {
            return Err(UploadError::rejected(
                ThreatClass::MimeMismatch,
                ReasonCode::ExtensionMagicMismatch,
            ));
        }
    }

    match format {
        CanonicalFormat::Pdf => preflight_pdf(file, limits)?,
        CanonicalFormat::Png
        | CanonicalFormat::Jpeg
        | CanonicalFormat::Webp
        | CanonicalFormat::Tiff
        | CanonicalFormat::Bmp => preflight_image(file, limits)?,
        CanonicalFormat::Wav
        | CanonicalFormat::Mp3
        | CanonicalFormat::Ogg
        | CanonicalFormat::Flac
        | CanonicalFormat::M4a => {
            if let Some(review) = preflight_audio(file, format, limits)? {
                return Ok(review);
            }
        }
        CanonicalFormat::Csv => {
            if let Some(review) = preflight_csv(file)? {
                return Ok(review);
            }
        }
        CanonicalFormat::Html => {
            if let Some(outcome) = preflight_html(file)? {
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

fn preflight_pdf(file: &mut File, limits: &LimitsConfig) -> Result<(), UploadError> {
    file.seek(SeekFrom::Start(0)).map_err(|_| {
        UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
    })?;
    // This is a bounded structural sniff only; accepted PDFs remain quarantined and the
    // converter stage performs deeper format validation.
    let mut buf = [0_u8; STREAM_CHUNK_BYTES];
    let mut overlap = Vec::new();
    let mut tail = Vec::new();
    let mut pages = 0_u32;
    let mut saw_header = false;
    let mut saw_startxref = false;
    let mut saw_xref_table = false;
    let mut saw_xref_stream = false;
    let mut saw_obj_stream = false;
    const TAIL_LIMIT: usize = 1024 * 1024;
    loop {
        let n = file.read(&mut buf).map_err(|_| {
            UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
        })?;
        if n == 0 {
            break;
        }
        if !saw_header {
            saw_header = buf[..n].starts_with(b"%PDF-");
        }
        let mut window = Vec::with_capacity(overlap.len() + n);
        window.extend_from_slice(&overlap);
        window.extend_from_slice(&buf[..n]);
        pages = pages.saturating_add(count_pdf_pages_heuristic(&window));
        saw_startxref |= window.windows(9).any(|w| w == b"startxref");
        saw_xref_table |= window.windows(5).any(|w| w == b"\nxref" || w == b"\rxref");
        saw_xref_stream |= window.windows(10).any(|w| w == b"/Type/XRef")
            || window.windows(11).any(|w| w == b"/Type /XRef");
        saw_obj_stream |= window.windows(7).any(|w| w == b"/ObjStm");
        if pages > limits.max_pdf_pages {
            return Err(UploadError::rejected(
                ThreatClass::PdfPageBomb,
                ReasonCode::PdfPageLimit,
            ));
        }
        tail.extend_from_slice(&buf[..n]);
        if tail.len() > TAIL_LIMIT {
            let excess = tail.len() - TAIL_LIMIT;
            tail.drain(..excess);
        }
        let keep = window.len().min(64);
        overlap.clear();
        overlap.extend_from_slice(&window[window.len() - keep..]);
    }
    let trimmed = trim_ascii_end(&tail);
    if !saw_header
        || !trimmed.ends_with(b"%%EOF")
        || !saw_startxref
        || !(saw_xref_table || saw_xref_stream || saw_obj_stream)
    {
        return Err(UploadError::rejected(
            ThreatClass::ParserCorruption,
            ReasonCode::PdfMissingEof,
        ));
    }
    Ok(())
}

fn count_pdf_pages_heuristic(data: &[u8]) -> u32 {
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

fn preflight_image(file: &mut File, limits: &LimitsConfig) -> Result<(), UploadError> {
    file.seek(SeekFrom::Start(0)).map_err(|_| {
        UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
    })?;
    let clone = file.try_clone().map_err(|_| {
        UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
    })?;
    let reader = image::ImageReader::new(BufReader::new(clone))
        .with_guessed_format()
        .map_err(|_| {
            UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
        })?;
    let (width, height) = reader.into_dimensions().map_err(|_| {
        UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
    })?;
    let pixels = u64::from(width).saturating_mul(u64::from(height));
    if pixels == 0 {
        return Err(UploadError::rejected(
            ThreatClass::ParserCorruption,
            ReasonCode::FailClosed,
        ));
    }
    if pixels > limits.max_image_pixels {
        return Err(UploadError::rejected(
            ThreatClass::ImagePixelBomb,
            ReasonCode::ImagePixelLimit,
        ));
    }
    Ok(())
}

fn preflight_audio(
    file: &mut File,
    format: CanonicalFormat,
    limits: &LimitsConfig,
) -> Result<Option<ValidationResult>, UploadError> {
    file.seek(SeekFrom::Start(0)).map_err(|_| {
        UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
    })?;
    let duration_secs = if format == CanonicalFormat::Wav {
        validate_wav_duration(file)?
    } else {
        validate_symphonia_duration(file, format)?
    };
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

fn validate_wav_duration(file: &mut File) -> Result<u64, UploadError> {
    file.seek(SeekFrom::Start(0)).map_err(|_| {
        UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
    })?;
    let file_len = file
        .metadata()
        .map_err(|_| UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed))?
        .len();
    let mut header = [0_u8; 12];
    file.read_exact(&mut header).map_err(|_| {
        UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
    })?;
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err(UploadError::rejected(
            ThreatClass::ParserCorruption,
            ReasonCode::FailClosed,
        ));
    }
    let riff_size = u32::from_le_bytes(header[4..8].try_into().unwrap_or([0; 4])) as u64;
    if riff_size < 4 || riff_size.saturating_add(8) > file_len {
        return Err(UploadError::rejected(
            ThreatClass::ParserCorruption,
            ReasonCode::FailClosed,
        ));
    }
    let mut byte_rate = None;
    let mut data_bytes = None;
    let mut scanned = 12_u64;
    let riff_end = riff_size.saturating_add(8).min(file_len);
    while scanned < 1024 * 1024 && scanned + 8 <= riff_end {
        let mut chunk = [0_u8; 8];
        file.read_exact(&mut chunk).map_err(|_| {
            UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
        })?;
        scanned = scanned.saturating_add(8);
        let id = &chunk[0..4];
        let len = u32::from_le_bytes(chunk[4..8].try_into().unwrap_or([0; 4])) as u64;
        let padded_len = len.saturating_add(len % 2);
        let chunk_data_end = scanned.saturating_add(len);
        let chunk_end = scanned.saturating_add(padded_len);
        if chunk_data_end > riff_end || chunk_end > file_len {
            return Err(UploadError::rejected(
                ThreatClass::ParserCorruption,
                ReasonCode::FailClosed,
            ));
        }
        if id == b"fmt " {
            let mut fmt = vec![0_u8; len.min(64) as usize];
            file.read_exact(&mut fmt).map_err(|_| {
                UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
            })?;
            if len > 64 {
                file.seek(SeekFrom::Current((len - 64) as i64))
                    .map_err(|_| {
                        UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
                    })?;
            }
            if fmt.len() < 16 {
                return Err(UploadError::rejected(
                    ThreatClass::ParserCorruption,
                    ReasonCode::FailClosed,
                ));
            }
            byte_rate = Some(u32::from_le_bytes(fmt[8..12].try_into().unwrap_or([0; 4])));
        } else if id == b"data" {
            data_bytes = Some(len);
            file.seek(SeekFrom::Current(len as i64)).map_err(|_| {
                UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
            })?;
        } else {
            file.seek(SeekFrom::Current(len as i64)).map_err(|_| {
                UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
            })?;
        }
        if padded_len > len {
            file.seek(SeekFrom::Current((padded_len - len) as i64))
                .map_err(|_| {
                    UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
                })?;
        }
        scanned = chunk_end;
        if byte_rate.is_some() && data_bytes.is_some() {
            break;
        }
    }
    let byte_rate = byte_rate.ok_or_else(|| {
        UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
    })?;
    let data_bytes = data_bytes.ok_or_else(|| {
        UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
    })?;
    if byte_rate == 0 || data_bytes == 0 {
        return Err(UploadError::rejected(
            ThreatClass::ParserCorruption,
            ReasonCode::FailClosed,
        ));
    }
    Ok(data_bytes / u64::from(byte_rate))
}

fn validate_symphonia_duration(
    file: &mut File,
    format: CanonicalFormat,
) -> Result<u64, UploadError> {
    // Non-WAV audio gets a bounded container probe here; full packet/decode validation is
    // intentionally left to the converter while the object remains quarantined.
    file.seek(SeekFrom::Start(0)).map_err(|_| {
        UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
    })?;
    let source = file.try_clone().map_err(|_| {
        UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
    })?;
    let mss = symphonia::core::io::MediaSourceStream::new(Box::new(source), Default::default());
    let mut hint = symphonia::core::probe::Hint::new();
    hint.with_extension(format.canonical_extension());
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &symphonia::core::formats::FormatOptions::default(),
            &symphonia::core::meta::MetadataOptions::default(),
        )
        .map_err(|_| {
            UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
        })?;
    let track = probed.format.default_track().ok_or_else(|| {
        UploadError::rejected(ThreatClass::ParserCorruption, ReasonCode::FailClosed)
    })?;
    let params = &track.codec_params;
    let duration = params
        .time_base
        .zip(params.n_frames)
        .map(|(base, frames)| base.calc_time(frames));
    let Some(duration) = duration else {
        return Err(UploadError::rejected(
            ThreatClass::ParserCorruption,
            ReasonCode::FailClosed,
        ));
    };
    if duration.seconds == 0 && duration.frac <= 0.0 {
        return Err(UploadError::rejected(
            ThreatClass::ParserCorruption,
            ReasonCode::FailClosed,
        ));
    }
    Ok(duration.seconds + u64::from(duration.frac > 0.0))
}

fn preflight_csv(file: &mut File) -> Result<Option<ValidationResult>, UploadError> {
    file.seek(SeekFrom::Start(0))
        .map_err(|_| UploadError::Internal)?;
    let mut buf = String::new();
    // Bound CSV preflight text read.
    let mut limited = file.take(1024 * 1024);
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

fn preflight_html(file: &mut File) -> Result<Option<ValidationResult>, UploadError> {
    file.seek(SeekFrom::Start(0))
        .map_err(|_| UploadError::Internal)?;
    let mut buf = String::new();
    let mut limited = file.take(1024 * 1024);
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
    declared_content_type: Option<&str>,
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
    let mut file = streamed.rewinded_file_clone()?;
    let result = validate_file(
        &mut file,
        &streamed.head,
        declared_filename,
        declared_content_type,
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
        let err = validate_streamed_bytes(
            &streamed,
            Some("plain-text.pdf"),
            Some("text/plain"),
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
