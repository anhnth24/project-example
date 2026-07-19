//! Bounded streaming intake: size cap + SHA-256 without full-file buffering.

use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::time::Duration;

use futures::StreamExt;
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use tokio::io::AsyncReadExt;

use super::error::{ReasonCode, ThreatClass, UploadError};
use super::limits::{LimitsConfig, MAGIC_SNIFF_BYTES, STREAM_CHUNK_BYTES};

/// Result of streaming an upload body to a temporary file.
#[derive(Debug)]
pub struct StreamedUpload {
    pub tempfile: NamedTempFile,
    pub sha256_hex: String,
    pub size_bytes: u64,
    pub head: Vec<u8>,
}

impl StreamedUpload {
    /// Clone and rewind the already-held tempfile descriptor; callers must not reopen by path.
    pub fn rewinded_file_clone(&self) -> Result<File, UploadError> {
        let mut file = self
            .tempfile
            .as_file()
            .try_clone()
            .map_err(|_| UploadError::Internal)?;
        file.seek(SeekFrom::Start(0))
            .map_err(|_| UploadError::Internal)?;
        Ok(file)
    }
}

/// Consume an async byte stream into a tempfile while hashing and enforcing the size cap.
///
/// Memory use is O(chunk size): never buffers the whole object. Aborts with
/// [`ThreatClass::Oversize`] as soon as the cap is exceeded.
pub async fn stream_to_tempfile<S, E>(
    mut stream: S,
    limits: &LimitsConfig,
) -> Result<StreamedUpload, UploadError>
where
    S: futures::Stream<Item = Result<bytes::Bytes, E>> + Unpin,
    E: std::fmt::Debug,
{
    let mut tempfile = NamedTempFile::new().map_err(|_| UploadError::Internal)?;
    let mut hasher = Sha256::new();
    let mut size_bytes: u64 = 0;
    let mut head = Vec::with_capacity(MAGIC_SNIFF_BYTES);

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| {
            UploadError::rejected(ThreatClass::TruncatedUpload, ReasonCode::StreamInterrupted)
        })?;
        if chunk.is_empty() {
            continue;
        }
        let next = size_bytes.saturating_add(chunk.len() as u64);
        if next > limits.max_upload_bytes {
            // Best-effort cleanup; NamedTempFile drops on return.
            let _ = tempfile.close();
            return Err(UploadError::rejected(
                ThreatClass::Oversize,
                ReasonCode::UploadTooLarge,
            ));
        }
        size_bytes = next;
        hasher.update(&chunk);
        if head.len() < MAGIC_SNIFF_BYTES {
            let need = MAGIC_SNIFF_BYTES - head.len();
            head.extend_from_slice(&chunk[..chunk.len().min(need)]);
        }
        tempfile
            .write_all(&chunk)
            .map_err(|_| UploadError::Internal)?;
    }

    tempfile.flush().map_err(|_| UploadError::Internal)?;
    let sha256_hex = hex::encode(hasher.finalize());
    Ok(StreamedUpload {
        tempfile,
        sha256_hex,
        size_bytes,
        head,
    })
}

/// Same as [`stream_to_tempfile`], with an idle timeout applied to each chunk wait.
pub async fn stream_to_tempfile_with_idle_timeout<S, E>(
    mut stream: S,
    limits: &LimitsConfig,
    idle_timeout: Duration,
) -> Result<StreamedUpload, UploadError>
where
    S: futures::Stream<Item = Result<bytes::Bytes, E>> + Unpin,
    E: std::fmt::Debug,
{
    let mut tempfile = NamedTempFile::new().map_err(|_| UploadError::Internal)?;
    let mut hasher = Sha256::new();
    let mut size_bytes: u64 = 0;
    let mut head = Vec::with_capacity(MAGIC_SNIFF_BYTES);

    loop {
        let chunk = tokio::time::timeout(idle_timeout, stream.next())
            .await
            .map_err(|_| UploadError::MultipartInvalid {
                reason: ReasonCode::MultipartTimeout,
            })?;
        let Some(chunk) = chunk else {
            break;
        };
        let chunk = chunk.map_err(|_| {
            UploadError::rejected(ThreatClass::TruncatedUpload, ReasonCode::StreamInterrupted)
        })?;
        if chunk.is_empty() {
            continue;
        }
        let next = size_bytes.saturating_add(chunk.len() as u64);
        if next > limits.max_upload_bytes {
            let _ = tempfile.close();
            return Err(UploadError::rejected(
                ThreatClass::Oversize,
                ReasonCode::UploadTooLarge,
            ));
        }
        size_bytes = next;
        hasher.update(&chunk);
        if head.len() < MAGIC_SNIFF_BYTES {
            let need = MAGIC_SNIFF_BYTES - head.len();
            head.extend_from_slice(&chunk[..chunk.len().min(need)]);
        }
        tempfile
            .write_all(&chunk)
            .map_err(|_| UploadError::Internal)?;
    }

    tempfile.flush().map_err(|_| UploadError::Internal)?;
    Ok(StreamedUpload {
        tempfile,
        sha256_hex: hex::encode(hasher.finalize()),
        size_bytes,
        head,
    })
}

/// Stream from an `AsyncRead` (e.g. multipart field) with the same bounds.
pub async fn stream_async_read_to_tempfile<R>(
    mut reader: R,
    limits: &LimitsConfig,
) -> Result<StreamedUpload, UploadError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut tempfile = NamedTempFile::new().map_err(|_| UploadError::Internal)?;
    let mut hasher = Sha256::new();
    let mut size_bytes: u64 = 0;
    let mut head = Vec::with_capacity(MAGIC_SNIFF_BYTES);
    let mut buf = vec![0_u8; STREAM_CHUNK_BYTES];

    loop {
        let n = reader.read(&mut buf).await.map_err(|_| {
            UploadError::rejected(ThreatClass::TruncatedUpload, ReasonCode::StreamInterrupted)
        })?;
        if n == 0 {
            break;
        }
        let chunk = &buf[..n];
        let next = size_bytes.saturating_add(n as u64);
        if next > limits.max_upload_bytes {
            let _ = tempfile.close();
            return Err(UploadError::rejected(
                ThreatClass::Oversize,
                ReasonCode::UploadTooLarge,
            ));
        }
        size_bytes = next;
        hasher.update(chunk);
        if head.len() < MAGIC_SNIFF_BYTES {
            let need = MAGIC_SNIFF_BYTES - head.len();
            head.extend_from_slice(&chunk[..chunk.len().min(need)]);
        }
        tempfile
            .write_all(chunk)
            .map_err(|_| UploadError::Internal)?;
    }

    tempfile.flush().map_err(|_| UploadError::Internal)?;
    Ok(StreamedUpload {
        tempfile,
        sha256_hex: hex::encode(hasher.finalize()),
        size_bytes,
        head,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use futures::stream;

    #[tokio::test]
    async fn rejects_oversize_before_buffering_all() {
        let limits = LimitsConfig {
            max_upload_bytes: 1_024,
            ..LimitsConfig::policy_defaults()
        };
        let chunks = (0..20).map(|_| Ok::<_, std::io::Error>(Bytes::from(vec![0_u8; 100])));
        let stream = stream::iter(chunks);
        let err = stream_to_tempfile(stream, &limits).await.unwrap_err();
        assert_eq!(err.threat_class(), Some(ThreatClass::Oversize));
        assert_eq!(err.status_code(), axum::http::StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn hashes_small_stream() {
        let limits = LimitsConfig::policy_defaults();
        let stream = stream::iter(vec![
            Ok::<_, std::io::Error>(Bytes::from_static(b"hello ")),
            Ok(Bytes::from_static(b"world")),
        ]);
        let uploaded = stream_to_tempfile(stream, &limits).await.unwrap();
        assert_eq!(uploaded.size_bytes, 11);
        assert_eq!(
            uploaded.sha256_hex,
            hex::encode(Sha256::digest(b"hello world"))
        );
        assert_eq!(&uploaded.head, b"hello world");
    }
}
