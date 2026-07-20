//! Upload security limits from `docs/markhand-web-upload-policy.md` §2.

/// Max upload object size: 200 MiB.
pub const MAX_UPLOAD_BYTES: u64 = 200 * 1024 * 1024;
/// Max archive entries: 4,096.
pub const MAX_ARCHIVE_ENTRIES: u64 = 4_096;
/// Max archive uncompressed bytes: 1 GiB.
pub const MAX_ARCHIVE_UNCOMPRESSED_BYTES: u64 = 1024 * 1024 * 1024;
/// Max archive compression ratio: 100:1.
pub const MAX_ARCHIVE_COMPRESSION_RATIO: u64 = 100;
/// Max PDF pages (header-level preflight).
pub const MAX_PDF_PAGES: u32 = 500;
/// Max image pixels (header-level preflight).
pub const MAX_IMAGE_PIXELS: u64 = 80_000_000;
/// Max audio duration seconds (header-level preflight).
pub const MAX_AUDIO_DURATION_SECS: u64 = 3_600;
/// Max multipart parts accepted per upload request.
pub const MAX_MULTIPART_PARTS: u32 = 8;
/// Max bytes allowed in per-part disposition/type metadata captured by Axum.
pub const MAX_PART_HEADER_BYTES: usize = 8 * 1024;
/// Whole upload request timeout.
pub const UPLOAD_TIMEOUT_SECS: u64 = 120;
/// Idle timeout while waiting for the next multipart field/chunk.
pub const UPLOAD_IDLE_TIMEOUT_SECS: u64 = 15;
/// Streaming read chunk size (bounded memory).
pub const STREAM_CHUNK_BYTES: usize = 64 * 1024;
/// Bytes retained from the head for magic sniffing.
pub const MAGIC_SNIFF_BYTES: usize = 512;
/// Bound for a single archive entry decompression during membership checks.
pub const MAX_SINGLE_ENTRY_READ_BYTES: u64 = 16 * 1024 * 1024;

/// Typed upload limits (policy defaults; overridable via config).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LimitsConfig {
    pub max_upload_bytes: u64,
    pub max_archive_entries: u64,
    pub max_archive_uncompressed_bytes: u64,
    pub max_archive_compression_ratio: u64,
    pub max_pdf_pages: u32,
    pub max_image_pixels: u64,
    pub max_audio_duration_secs: u64,
    pub max_multipart_parts: u32,
    pub max_part_header_bytes: usize,
    pub upload_timeout_secs: u64,
    pub upload_idle_timeout_secs: u64,
}

impl LimitsConfig {
    /// Policy defaults from P0-09 upload policy §2.
    pub const fn policy_defaults() -> Self {
        Self {
            max_upload_bytes: MAX_UPLOAD_BYTES,
            max_archive_entries: MAX_ARCHIVE_ENTRIES,
            max_archive_uncompressed_bytes: MAX_ARCHIVE_UNCOMPRESSED_BYTES,
            max_archive_compression_ratio: MAX_ARCHIVE_COMPRESSION_RATIO,
            max_pdf_pages: MAX_PDF_PAGES,
            max_image_pixels: MAX_IMAGE_PIXELS,
            max_audio_duration_secs: MAX_AUDIO_DURATION_SECS,
            max_multipart_parts: MAX_MULTIPART_PARTS,
            max_part_header_bytes: MAX_PART_HEADER_BYTES,
            upload_timeout_secs: UPLOAD_TIMEOUT_SECS,
            upload_idle_timeout_secs: UPLOAD_IDLE_TIMEOUT_SECS,
        }
    }

    /// Fail-closed validation of configured limits (must not exceed policy caps).
    pub fn validate(&self) -> Result<(), String> {
        if self.max_upload_bytes == 0 || self.max_upload_bytes > MAX_UPLOAD_BYTES {
            return Err(format!(
                "upload max_upload_bytes must be between 1 and {MAX_UPLOAD_BYTES}"
            ));
        }
        if self.max_archive_entries == 0 || self.max_archive_entries > MAX_ARCHIVE_ENTRIES {
            return Err(format!(
                "upload max_archive_entries must be between 1 and {MAX_ARCHIVE_ENTRIES}"
            ));
        }
        if self.max_archive_uncompressed_bytes == 0
            || self.max_archive_uncompressed_bytes > MAX_ARCHIVE_UNCOMPRESSED_BYTES
        {
            return Err(format!(
                "upload max_archive_uncompressed_bytes must be between 1 and {MAX_ARCHIVE_UNCOMPRESSED_BYTES}"
            ));
        }
        if self.max_archive_compression_ratio == 0
            || self.max_archive_compression_ratio > MAX_ARCHIVE_COMPRESSION_RATIO
        {
            return Err(format!(
                "upload max_archive_compression_ratio must be between 1 and {MAX_ARCHIVE_COMPRESSION_RATIO}"
            ));
        }
        if self.max_pdf_pages == 0 || self.max_pdf_pages > MAX_PDF_PAGES {
            return Err(format!(
                "upload max_pdf_pages must be between 1 and {MAX_PDF_PAGES}"
            ));
        }
        if self.max_image_pixels == 0 || self.max_image_pixels > MAX_IMAGE_PIXELS {
            return Err(format!(
                "upload max_image_pixels must be between 1 and {MAX_IMAGE_PIXELS}"
            ));
        }
        if self.max_audio_duration_secs == 0
            || self.max_audio_duration_secs > MAX_AUDIO_DURATION_SECS
        {
            return Err(format!(
                "upload max_audio_duration_secs must be between 1 and {MAX_AUDIO_DURATION_SECS}"
            ));
        }
        if self.max_multipart_parts == 0 || self.max_multipart_parts > MAX_MULTIPART_PARTS {
            return Err(format!(
                "upload max_multipart_parts must be between 1 and {MAX_MULTIPART_PARTS}"
            ));
        }
        if self.max_part_header_bytes == 0 || self.max_part_header_bytes > MAX_PART_HEADER_BYTES {
            return Err(format!(
                "upload max_part_header_bytes must be between 1 and {MAX_PART_HEADER_BYTES}"
            ));
        }
        if self.upload_timeout_secs == 0 || self.upload_timeout_secs > UPLOAD_TIMEOUT_SECS {
            return Err(format!(
                "upload upload_timeout_secs must be between 1 and {UPLOAD_TIMEOUT_SECS}"
            ));
        }
        if self.upload_idle_timeout_secs == 0
            || self.upload_idle_timeout_secs > UPLOAD_IDLE_TIMEOUT_SECS
            || self.upload_idle_timeout_secs > self.upload_timeout_secs
        {
            return Err(format!(
                "upload upload_idle_timeout_secs must be between 1 and min(upload_timeout_secs, {UPLOAD_IDLE_TIMEOUT_SECS})"
            ));
        }
        Ok(())
    }
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self::policy_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_defaults_match_documented_caps() {
        let limits = LimitsConfig::policy_defaults();
        assert_eq!(limits.max_upload_bytes, 200 * 1024 * 1024);
        assert_eq!(limits.max_archive_entries, 4_096);
        assert_eq!(limits.max_archive_uncompressed_bytes, 1024 * 1024 * 1024);
        assert_eq!(limits.max_archive_compression_ratio, 100);
        assert_eq!(limits.max_pdf_pages, 500);
        assert_eq!(limits.max_image_pixels, 80_000_000);
        assert_eq!(limits.max_audio_duration_secs, 3_600);
        assert_eq!(limits.max_multipart_parts, 8);
        assert_eq!(limits.max_part_header_bytes, 8 * 1024);
        assert_eq!(limits.upload_timeout_secs, 120);
        assert_eq!(limits.upload_idle_timeout_secs, 15);
        limits.validate().unwrap();
    }

    #[test]
    fn rejects_limits_above_policy_caps() {
        let mut limits = LimitsConfig::policy_defaults();
        limits.max_upload_bytes = MAX_UPLOAD_BYTES + 1;
        assert!(limits.validate().is_err());
    }
}
