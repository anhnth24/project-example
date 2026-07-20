//! Short-lived, one-time download capabilities for original quarantine objects.
//!
//! The consumed-nonce cache is process-local for the POC. Strict cross-instance
//! one-time-use requires a durable nonce ledger in a later HA hardening issue.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use base64::Engine;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use deadpool_postgres::Pool;
use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::auth::permissions::{require_permission, resolve_org_context_in_txn};
use crate::config::SecretString;
use crate::db::error::DbError;
use crate::db::pool::with_org_txn_typed;
use crate::services::audit::{record_audit_event, SafeAuditEvent};
use crate::storage::keys::parse_key_for_org;
use crate::storage::minio::MinioClient;
use crate::storage::{ObjectNamespace, StorageError};

const CAPABILITY_DOMAIN: &[u8] = b"markhand-download-capability-v1";
const CAPABILITY_KEY_DOMAIN: &[u8] = b"markhand-download-capability-key-v1";
const TOKEN_VERSION: u8 = 1;
const TOKEN_TTL_SECS: i64 = 60;
const TOKEN_BYTES_WITHOUT_TAG: usize = 1 + 16 + 16 + 16 + 16 + 16 + 8;
const TOKEN_BYTES: usize = TOKEN_BYTES_WITHOUT_TAG + 32;
const NONCE_CACHE_GRACE: Duration = Duration::from_secs(60);

#[derive(Clone, PartialEq, Eq)]
pub struct CapabilityKey([u8; 32]);

impl std::fmt::Debug for CapabilityKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CapabilityKey([REDACTED])")
    }
}

impl CapabilityKey {
    pub fn derive_from_auth_signing_key(signing_key: &SecretString) -> Self {
        Self(hmac_sha256(
            signing_key.expose().as_bytes(),
            CAPABILITY_KEY_DOMAIN,
        ))
    }

    pub fn sign_domain_separated(&self, domain: &[u8], payload: &[u8]) -> [u8; 32] {
        let domain_len = u32::try_from(domain.len()).unwrap_or(u32::MAX);
        let mut message = Vec::with_capacity(4 + domain.len() + payload.len());
        message.extend_from_slice(&domain_len.to_be_bytes());
        message.extend_from_slice(domain);
        message.extend_from_slice(payload);
        hmac_sha256(&self.0, &message)
    }

    pub fn verify_domain_separated(&self, domain: &[u8], payload: &[u8], tag: &[u8]) -> bool {
        let expected = self.sign_domain_separated(domain, payload);
        constant_time_eq(tag, &expected)
    }

    #[cfg(test)]
    fn from_test_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

#[derive(Debug, Default)]
pub struct ConsumedDownloadNonces {
    inner: Mutex<HashMap<Uuid, Instant>>,
}

impl ConsumedDownloadNonces {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn consume_once(
        &self,
        nonce: Uuid,
        expires_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<(), DownloadError> {
        let mut guard = self.inner.lock().await;
        let instant_now = Instant::now();
        guard.retain(|_, expires| *expires > instant_now);
        if guard.contains_key(&nonce) {
            return Err(DownloadError::Replay);
        }
        let ttl = expires_at
            .signed_duration_since(now)
            .to_std()
            .unwrap_or(Duration::ZERO)
            .saturating_add(NONCE_CACHE_GRACE);
        guard.insert(nonce, instant_now + ttl);
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadCapability {
    pub token: String,
    pub download_path: String,
    pub expires_at: DateTime<Utc>,
    pub filename: String,
    pub content_type: String,
    pub byte_size: u64,
    pub content_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadStream {
    pub bytes: Bytes,
    pub content_type: String,
    pub filename: String,
    pub byte_size: u64,
    pub content_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafeDownloadFilename {
    pub filename: String,
    pub ascii_fallback: String,
    pub filename_star: String,
}

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("download source was not found")]
    NotFound,
    #[error("download capability service is unavailable")]
    CapabilityUnavailable,
    #[error("download capability token is invalid")]
    InvalidToken,
    #[error("download capability token is expired")]
    Expired,
    #[error("download capability token was already used")]
    Replay,
    #[error("database error")]
    Db(#[from] DbError),
    #[error("storage error")]
    Storage(#[from] StorageError),
    #[error("download object integrity check failed")]
    Integrity,
}

#[derive(Debug, Clone)]
struct AuthorizedDownload {
    org_id: Uuid,
    user_id: Uuid,
    document_id: Uuid,
    version_id: Uuid,
    original_object_key: String,
    source_filename: Option<String>,
    byte_size: Option<i64>,
}

#[derive(Debug, Clone)]
struct VerifiedToken {
    org_id: Uuid,
    user_id: Uuid,
    document_id: Uuid,
    version_id: Uuid,
    nonce: Uuid,
    expires_at: DateTime<Utc>,
}

pub async fn authorize_download(
    pool: &Pool,
    storage: &MinioClient,
    ctx: &OrgContext,
    document_id: Uuid,
    version_id: Uuid,
    capability_key: &CapabilityKey,
    now: DateTime<Utc>,
) -> Result<DownloadCapability, DownloadError> {
    let source = authorize_original_source(pool, ctx, document_id, version_id).await?;
    let key = parse_key_for_org(&source.original_object_key, ctx.org_id())?;
    if key.namespace() != ObjectNamespace::Quarantine {
        return Err(DownloadError::NotFound);
    }
    let metadata = storage.head_metadata(ctx.org_id(), &key).await?;
    let object_sha256 =
        metadata_value(&metadata, "content-sha256").ok_or(DownloadError::Integrity)?;
    let byte_size = match byte_size_from_metadata(&metadata) {
        Some(result) => result?,
        None => db_byte_size(source.byte_size)?,
    };
    let content_type = content_type_from_metadata(&metadata);
    let safe_name = sanitize_download_filename(source.source_filename.as_deref(), document_id);
    let expires_at = now + chrono::Duration::seconds(TOKEN_TTL_SECS);
    let token = encode_token(
        capability_key,
        TokenClaims {
            org_id: source.org_id,
            user_id: source.user_id,
            document_id: source.document_id,
            version_id: source.version_id,
            nonce: Uuid::new_v4(),
            expires_at,
        },
    );

    Ok(DownloadCapability {
        download_path: format!("/api/v1/documents/download/{token}"),
        token,
        expires_at,
        filename: safe_name.filename,
        content_type,
        byte_size,
        content_sha256: object_sha256.to_string(),
    })
}

pub async fn redeem_download(
    pool: &Pool,
    storage: &MinioClient,
    capability_key: &CapabilityKey,
    consumed_nonces: &ConsumedDownloadNonces,
    token: &str,
    request_id: &str,
    now: DateTime<Utc>,
) -> Result<DownloadStream, DownloadError> {
    let verified = verify_token(capability_key, token, now)?;
    consumed_nonces
        .consume_once(verified.nonce, verified.expires_at, now)
        .await?;
    let ctx = match resolve_org_context_in_txn(pool, verified.org_id, verified.user_id).await {
        Ok(ctx) => ctx,
        Err(_) => {
            if let Ok(ctx) =
                OrgContext::try_new(verified.org_id, verified.user_id, [] as [&str; 0], [])
            {
                warn_audit_failure(
                    audit_redeem(
                        pool,
                        &ctx,
                        &verified,
                        "deny",
                        Some("identity_not_authorized"),
                        request_id,
                        None,
                    )
                    .await,
                    "deny",
                    request_id,
                );
            }
            return Err(DownloadError::NotFound);
        }
    };
    if require_permission(&ctx, "qa.query").is_err() {
        warn_audit_failure(
            audit_redeem(
                pool,
                &ctx,
                &verified,
                "deny",
                Some("permission_denied"),
                request_id,
                None,
            )
            .await,
            "deny",
            request_id,
        );
        return Err(DownloadError::NotFound);
    }
    let source = match authorize_original_source(
        pool,
        &ctx,
        verified.document_id,
        verified.version_id,
    )
    .await
    {
        Ok(source) => source,
        Err(error) => {
            let outcome = download_audit_outcome(&error);
            warn_audit_failure(
                audit_redeem(
                    pool,
                    &ctx,
                    &verified,
                    outcome,
                    Some(download_audit_reason(&error)),
                    request_id,
                    None,
                )
                .await,
                outcome,
                request_id,
            );
            return Err(error);
        }
    };
    let key = parse_key_for_org(&source.original_object_key, ctx.org_id())?;
    if key.namespace() != ObjectNamespace::Quarantine {
        warn_audit_failure(
            audit_redeem(
                pool,
                &ctx,
                &verified,
                "deny",
                Some("source_not_found"),
                request_id,
                None,
            )
            .await,
            "deny",
            request_id,
        );
        return Err(DownloadError::NotFound);
    }
    let metadata = storage.head_metadata(ctx.org_id(), &key).await?;
    let content_type = content_type_from_metadata(&metadata);
    let filename =
        sanitize_download_filename(source.source_filename.as_deref(), source.document_id).filename;
    let expected_sha256 =
        metadata_value(&metadata, "content-sha256").ok_or(DownloadError::Integrity)?;
    let bytes = storage.get_object(ctx.org_id(), &key).await?;
    let actual_sha256 = hex::encode(Sha256::digest(&bytes));
    if actual_sha256 != expected_sha256 {
        warn_audit_failure(
            audit_redeem(
                pool,
                &ctx,
                &verified,
                "error",
                Some("integrity_failed"),
                request_id,
                None,
            )
            .await,
            "error",
            request_id,
        );
        return Err(DownloadError::Integrity);
    }
    let byte_size = bytes.len() as u64;
    warn_audit_failure(
        audit_redeem(
            pool,
            &ctx,
            &verified,
            "success",
            None,
            request_id,
            Some(byte_size),
        )
        .await,
        "success",
        request_id,
    );
    Ok(DownloadStream {
        byte_size,
        bytes,
        content_type,
        filename,
        content_sha256: actual_sha256,
    })
}

async fn audit_redeem(
    pool: &Pool,
    ctx: &OrgContext,
    verified: &VerifiedToken,
    outcome: &'static str,
    reason: Option<&'static str>,
    request_id: &str,
    byte_size: Option<u64>,
) -> Result<(), DbError> {
    let mut metadata = serde_json::json!({
        "documentId": verified.document_id.to_string(),
        "versionId": verified.version_id.to_string()
    });
    if let Some(reason) = reason {
        metadata["reason"] = serde_json::Value::String(reason.into());
    }
    if let Some(byte_size) = byte_size {
        metadata["byteSize"] = serde_json::Value::Number(byte_size.into());
    }
    record_audit_event(
        pool,
        ctx,
        SafeAuditEvent {
            action: "document.download.redeem",
            resource_type: "document_version",
            resource_id: Some(verified.version_id.to_string()),
            outcome,
            request_id: request_id.into(),
            metadata,
        },
    )
    .await
}

fn warn_audit_failure(result: Result<(), DbError>, outcome: &'static str, request_id: &str) {
    if let Err(error) = result {
        tracing::warn!(
            action = "document.download.redeem",
            outcome = outcome,
            request_id = %request_id,
            error_code = error.code(),
            "audit write failed"
        );
    }
}

fn download_audit_outcome(error: &DownloadError) -> &'static str {
    match error {
        DownloadError::NotFound
        | DownloadError::InvalidToken
        | DownloadError::Expired
        | DownloadError::Replay => "deny",
        DownloadError::CapabilityUnavailable
        | DownloadError::Db(_)
        | DownloadError::Storage(_)
        | DownloadError::Integrity => "error",
    }
}

fn download_audit_reason(error: &DownloadError) -> &'static str {
    match error {
        DownloadError::NotFound => "source_not_found",
        DownloadError::CapabilityUnavailable => "capability_unavailable",
        DownloadError::InvalidToken => "invalid_token",
        DownloadError::Expired => "expired",
        DownloadError::Replay => "replay",
        DownloadError::Db(_) => "database_error",
        DownloadError::Storage(_) => "storage_error",
        DownloadError::Integrity => "integrity_failed",
    }
}

async fn authorize_original_source(
    pool: &Pool,
    ctx: &OrgContext,
    document_id: Uuid,
    version_id: Uuid,
) -> Result<AuthorizedDownload, DownloadError> {
    if ctx.org_id().is_nil() || ctx.allowed_collection_ids().is_empty() {
        return Err(DownloadError::NotFound);
    }
    let authorized = ctx
        .allowed_collection_ids()
        .iter()
        .copied()
        .collect::<Vec<_>>();
    let txn_ctx = ctx.clone();
    let query_ctx = txn_ctx.clone();
    with_org_txn_typed(pool, &txn_ctx, move |txn| {
        Box::pin(async move {
            let row = txn
                .query_opt(
                    "SELECT v.document_id,
                            v.id AS version_id,
                            v.original_object_key,
                            v.source_filename,
                            v.byte_size
                     FROM document_versions v
                     JOIN documents d
                       ON d.org_id = v.org_id
                      AND d.id = v.document_id
                     WHERE v.org_id = $1
                       AND v.document_id = $3
                       AND v.id = $4
                       AND d.collection_id = ANY($2::uuid[])
                       AND d.state = 'indexed'
                       AND d.deleted_at IS NULL",
                    &[&query_ctx.org_id(), &authorized, &document_id, &version_id],
                )
                .await
                .map_err(DbError::from)?
                .ok_or(DownloadError::NotFound)?;
            Ok::<_, DownloadError>(AuthorizedDownload {
                org_id: query_ctx.org_id(),
                user_id: query_ctx.user_id(),
                document_id: row.get("document_id"),
                version_id: row.get("version_id"),
                original_object_key: row.get("original_object_key"),
                source_filename: row.get("source_filename"),
                byte_size: row.get("byte_size"),
            })
        })
    })
    .await
}

fn metadata_value<'a>(metadata: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    metadata
        .get(key)
        .or_else(|| metadata.get(&format!("x-amz-meta-{key}")))
        .map(String::as_str)
}

fn byte_size_from_metadata(
    metadata: &HashMap<String, String>,
) -> Option<Result<u64, DownloadError>> {
    metadata_value(metadata, "content-length-bytes")
        .or_else(|| metadata_value(metadata, "content-length"))
        .map(|value| value.parse::<u64>().map_err(|_| DownloadError::Integrity))
}

fn db_byte_size(byte_size: Option<i64>) -> Result<u64, DownloadError> {
    let byte_size = byte_size.ok_or(DownloadError::Integrity)?;
    u64::try_from(byte_size).map_err(|_| DownloadError::Integrity)
}

fn content_type_from_metadata(metadata: &HashMap<String, String>) -> String {
    if let Some(content_type) = metadata_value(metadata, "content-type")
        .filter(|value| !value.trim().is_empty())
        .filter(|value| !value.contains('\r') && !value.contains('\n'))
    {
        return content_type.to_string();
    }
    metadata_value(metadata, "canonical-format")
        .and_then(content_type_for_format)
        .unwrap_or("application/octet-stream")
        .to_string()
}

fn content_type_for_format(format: &str) -> Option<&'static str> {
    match format {
        "pdf" => Some("application/pdf"),
        "docx" => Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
        "pptx" => Some("application/vnd.openxmlformats-officedocument.presentationml.presentation"),
        "xlsx" => Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
        "ods" => Some("application/vnd.oasis.opendocument.spreadsheet"),
        "xls" => Some("application/vnd.ms-excel"),
        "xlsb" => Some("application/vnd.ms-excel.sheet.binary.macroEnabled.12"),
        "csv" => Some("text/csv; charset=utf-8"),
        "html" => Some("text/html; charset=utf-8"),
        "txt" => Some("text/plain; charset=utf-8"),
        "png" => Some("image/png"),
        "jpeg" | "jpg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "tiff" => Some("image/tiff"),
        "bmp" => Some("image/bmp"),
        "wav" => Some("audio/wav"),
        "mp3" => Some("audio/mpeg"),
        "ogg" => Some("audio/ogg"),
        "flac" => Some("audio/flac"),
        "m4a" => Some("audio/mp4"),
        "zip" => Some("application/zip"),
        _ => None,
    }
}

pub fn sanitize_download_filename(input: Option<&str>, document_id: Uuid) -> SafeDownloadFilename {
    let fallback = format!("document-{document_id}");
    let mut filename = input
        .unwrap_or("")
        .chars()
        .filter(|ch| !ch.is_control() && !matches!(*ch, '/' | '\\' | '"' | ';'))
        .collect::<String>();
    filename = filename.trim().to_string();
    if filename.is_empty() || filename == "." || filename == ".." {
        filename = fallback.clone();
    }
    filename = filename.chars().take(180).collect();
    if filename.is_empty() {
        filename = fallback.clone();
    }
    let mut ascii_fallback = filename
        .chars()
        .filter(|ch| ch.is_ascii() && !ch.is_control() && !matches!(*ch, '"' | '\\' | ';'))
        .collect::<String>()
        .trim()
        .to_string();
    if ascii_fallback.is_empty() || ascii_fallback == "." || ascii_fallback == ".." {
        ascii_fallback = fallback;
    }
    ascii_fallback = ascii_fallback.chars().take(120).collect();
    let filename_star = percent_encode_filename(&filename);
    SafeDownloadFilename {
        filename,
        ascii_fallback,
        filename_star,
    }
}

pub fn content_disposition_value(filename: &str) -> String {
    let safe = sanitize_download_filename(Some(filename), Uuid::nil());
    format!(
        "attachment; filename=\"{}\"; filename*=UTF-8''{}",
        safe.ascii_fallback, safe.filename_star
    )
}

fn percent_encode_filename(filename: &str) -> String {
    let mut encoded = String::new();
    for byte in filename.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'_' | b'-') {
            encoded.push(*byte as char);
        } else {
            encoded.push('%');
            encoded.push_str(&format!("{byte:02X}"));
        }
    }
    encoded
}

#[derive(Debug, Clone, Copy)]
struct TokenClaims {
    org_id: Uuid,
    user_id: Uuid,
    document_id: Uuid,
    version_id: Uuid,
    nonce: Uuid,
    expires_at: DateTime<Utc>,
}

fn encode_token(key: &CapabilityKey, claims: TokenClaims) -> String {
    let mut payload = token_payload_without_tag(claims);
    let tag = capability_tag(key, &payload);
    payload.extend_from_slice(&tag);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload)
}

fn verify_token(
    key: &CapabilityKey,
    token: &str,
    now: DateTime<Utc>,
) -> Result<VerifiedToken, DownloadError> {
    if token.len() > 256 {
        return Err(DownloadError::InvalidToken);
    }
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|_| DownloadError::InvalidToken)?;
    if decoded.len() != TOKEN_BYTES || decoded[0] != TOKEN_VERSION {
        return Err(DownloadError::InvalidToken);
    }
    let (payload, tag) = decoded.split_at(TOKEN_BYTES_WITHOUT_TAG);
    let expected = capability_tag(key, payload);
    if !constant_time_eq(tag, &expected) {
        return Err(DownloadError::InvalidToken);
    }
    let claims = parse_claims(payload)?;
    if now.timestamp() >= claims.expires_at.timestamp() {
        return Err(DownloadError::Expired);
    }
    Ok(VerifiedToken {
        org_id: claims.org_id,
        user_id: claims.user_id,
        document_id: claims.document_id,
        version_id: claims.version_id,
        nonce: claims.nonce,
        expires_at: claims.expires_at,
    })
}

fn token_payload_without_tag(claims: TokenClaims) -> Vec<u8> {
    let mut payload = Vec::with_capacity(TOKEN_BYTES_WITHOUT_TAG);
    payload.push(TOKEN_VERSION);
    payload.extend_from_slice(claims.nonce.as_bytes());
    payload.extend_from_slice(claims.org_id.as_bytes());
    payload.extend_from_slice(claims.user_id.as_bytes());
    payload.extend_from_slice(claims.document_id.as_bytes());
    payload.extend_from_slice(claims.version_id.as_bytes());
    payload.extend_from_slice(&claims.expires_at.timestamp().to_be_bytes());
    payload
}

fn parse_claims(payload: &[u8]) -> Result<TokenClaims, DownloadError> {
    if payload.len() != TOKEN_BYTES_WITHOUT_TAG || payload[0] != TOKEN_VERSION {
        return Err(DownloadError::InvalidToken);
    }
    let nonce = uuid_from_slice(&payload[1..17])?;
    let org_id = uuid_from_slice(&payload[17..33])?;
    let user_id = uuid_from_slice(&payload[33..49])?;
    let document_id = uuid_from_slice(&payload[49..65])?;
    let version_id = uuid_from_slice(&payload[65..81])?;
    let mut exp_bytes = [0_u8; 8];
    exp_bytes.copy_from_slice(&payload[81..89]);
    let expires_at = DateTime::<Utc>::from_timestamp(i64::from_be_bytes(exp_bytes), 0)
        .ok_or(DownloadError::InvalidToken)?;
    Ok(TokenClaims {
        org_id,
        user_id,
        document_id,
        version_id,
        nonce,
        expires_at,
    })
}

fn uuid_from_slice(bytes: &[u8]) -> Result<Uuid, DownloadError> {
    let mut array = [0_u8; 16];
    array.copy_from_slice(bytes);
    let id = Uuid::from_bytes(array);
    if id.is_nil() {
        return Err(DownloadError::InvalidToken);
    }
    Ok(id)
}

fn capability_tag(key: &CapabilityKey, payload: &[u8]) -> [u8; 32] {
    let mut message = Vec::with_capacity(CAPABILITY_DOMAIN.len() + payload.len());
    message.extend_from_slice(CAPABILITY_DOMAIN);
    message.extend_from_slice(payload);
    hmac_sha256(&key.0, &message)
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut key_block = [0_u8; BLOCK];
    if key.len() > BLOCK {
        key_block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36_u8; BLOCK];
    let mut opad = [0x5c_u8; BLOCK];
    for index in 0..BLOCK {
        ipad[index] ^= key_block[index];
        opad[index] ^= key_block[index];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(message);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner);
    outer.finalize().into()
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0_u8;
    for (left, right) in left.iter().zip(right) {
        diff |= left ^ right;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use chrono::Duration as ChronoDuration;

    use super::*;

    #[test]
    fn sanitizes_download_filename_and_builds_rfc5987_header() {
        let document_id = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
        let safe = sanitize_download_filename(Some("../Báo cáo\nQ1;final.pdf"), document_id);
        assert_eq!(safe.filename, "..Báo cáoQ1final.pdf");
        assert_eq!(safe.ascii_fallback, "..Bo coQ1final.pdf");
        assert!(safe.filename_star.contains("%C3%A1"));
        let header = content_disposition_value(&safe.filename);
        assert!(
            header.starts_with("attachment; filename=\"..Bo coQ1final.pdf\"; filename*=UTF-8''")
        );
        assert!(!header.contains('\n'));

        let fallback = sanitize_download_filename(Some("////"), document_id);
        assert_eq!(fallback.filename, format!("document-{document_id}"));
    }

    #[test]
    fn hmac_token_validates_expiry_and_tampering() {
        let key = CapabilityKey::from_test_bytes([7; 32]);
        let now = Utc::now();
        let claims = TokenClaims {
            org_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            document_id: Uuid::new_v4(),
            version_id: Uuid::new_v4(),
            nonce: Uuid::new_v4(),
            expires_at: now + ChronoDuration::seconds(60),
        };
        let token = encode_token(&key, claims);
        let verified = verify_token(&key, &token, now).expect("valid token");
        assert_eq!(verified.org_id, claims.org_id);
        assert_eq!(verified.version_id, claims.version_id);

        let mut tampered = token.clone();
        tampered.push('A');
        assert!(matches!(
            verify_token(&key, &tampered, now),
            Err(DownloadError::InvalidToken)
        ));
        assert!(matches!(
            verify_token(&key, &token, now + ChronoDuration::seconds(61)),
            Err(DownloadError::Expired)
        ));
    }
}
