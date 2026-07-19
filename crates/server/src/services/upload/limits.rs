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
        limits.validate().unwrap();
    }

    #[test]
    fn rejects_limits_above_policy_caps() {
        let mut limits = LimitsConfig::policy_defaults();
        limits.max_upload_bytes = MAX_UPLOAD_BYTES + 1;
        assert!(limits.validate().is_err());
    }
}
