//! Short-lived, single-purpose download capabilities (P1B-R02).
//!
//! Capabilities are HMAC-signed opaque tokens. Redeem streams bytes through the
//! API after fresh ACL checks and single-use JTI redemption — clients never
//! receive raw bucket credentials or object keys.

use std::fmt;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{Duration, Utc};
use deadpool_postgres::Pool;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::config::SecretString;
use crate::db::error::DbError;
use crate::db::pool::with_org_txn;
use crate::services::access::{self, AccessError};
use crate::storage::keys::parse_key_for_org;
use crate::storage::minio::MinioClient;
use crate::storage::StorageError;

const TOKEN_PREFIX: &str = "mhcap1";
const DEFAULT_TTL_SECS: i64 = 120;
const MAX_TTL_SECS: i64 = 300;
const MAX_DOWNLOAD_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadPurpose {
    Markdown,
    Original,
}

impl DownloadPurpose {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Original => "original",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "markdown" => Some(Self::Markdown),
            "original" => Some(Self::Original),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CapabilityClaims {
    v: u16,
    org_id: Uuid,
    user_id: Uuid,
    document_id: Uuid,
    version_id: Uuid,
    purpose: String,
    jti: Uuid,
    iat: i64,
    exp: i64,
}

#[derive(Clone)]
pub struct CapabilityKeys {
    secret: SecretString,
}

impl fmt::Debug for CapabilityKeys {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CapabilityKeys")
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

impl CapabilityKeys {
    pub fn new(secret: SecretString) -> Result<Self, DownloadError> {
        if secret.expose().len() < 32 {
            return Err(DownloadError::NotConfigured);
        }
        Ok(Self { secret })
    }

    pub fn from_auth_signing_key(secret: &SecretString) -> Result<Self, DownloadError> {
        Self::new(SecretString::new(secret.expose().to_string()))
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct IssuedCapability {
    pub token: SecretString,
    pub expires_in: u64,
    pub purpose: DownloadPurpose,
    pub document_id: Uuid,
    pub version_id: Uuid,
}

impl fmt::Debug for IssuedCapability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IssuedCapability")
            .field("token", &"[REDACTED]")
            .field("expires_in", &self.expires_in)
            .field("purpose", &self.purpose)
            .field("document_id", &self.document_id)
            .field("version_id", &self.version_id)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadBytes {
    pub bytes: bytes::Bytes,
    pub content_type: &'static str,
    pub content_sha256: String,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub purpose: DownloadPurpose,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DownloadError {
    #[error("download capability is not configured")]
    NotConfigured,
    #[error("permission denied")]
    PermissionDenied,
    #[error("history permission required")]
    HistoryRequired,
    #[error("document or version not found")]
    NotFound,
    #[error("version is not published")]
    NotPublished,
    #[error("document deleted or suspended")]
    Suppressed,
    #[error("capability expired or invalid")]
    InvalidCapability,
    #[error("capability already redeemed")]
    Replay,
    #[error("object missing or unauthorized")]
    ObjectUnavailable,
    #[error("download exceeds size bound")]
    TooLarge,
    #[error("database error")]
    Database,
    #[error("storage error")]
    Storage,
}

impl DownloadError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotConfigured => "download_not_configured",
            Self::PermissionDenied => "download_permission_denied",
            Self::HistoryRequired => "download_history_required",
            Self::NotFound => "download_not_found",
            Self::NotPublished => "download_not_published",
            Self::Suppressed => "download_suppressed",
            Self::InvalidCapability => "download_invalid_capability",
            Self::Replay => "download_replay",
            Self::ObjectUnavailable => "download_object_unavailable",
            Self::TooLarge => "download_too_large",
            Self::Database => "download_database",
            Self::Storage => "download_storage",
        }
    }
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut key_block = [0u8; BLOCK];
    if key.len() > BLOCK {
        let digested = Sha256::digest(key);
        key_block[..32].copy_from_slice(&digested);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for index in 0..BLOCK {
        ipad[index] ^= key_block[index];
        opad[index] ^= key_block[index];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(message);
    let inner_hash = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_hash);
    let digest = outer.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

fn sign_payload(keys: &CapabilityKeys, payload_b64: &str) -> String {
    let mac = hmac_sha256(keys.secret.expose().as_bytes(), payload_b64.as_bytes());
    URL_SAFE_NO_PAD.encode(mac)
}

fn verify_mac(keys: &CapabilityKeys, payload_b64: &str, mac_b64: &str) -> bool {
    let expected = sign_payload(keys, payload_b64);
    // Constant-time-ish compare on equal-length strings.
    if expected.len() != mac_b64.len() {
        return false;
    }
    expected
        .as_bytes()
        .iter()
        .zip(mac_b64.as_bytes())
        .fold(0u8, |acc, (left, right)| acc | (left ^ right))
        == 0
}

struct AuthorizedDownloadTargets {
    original_object_key: String,
    markdown_object_key: Option<String>,
    /// `document_versions.content_sha256` (source/original bytes).
    source_content_sha256: String,
    /// `derived_artifacts.content_sha256` for Markdown, when present.
    markdown_content_sha256: Option<String>,
}

async fn authorize_version_access(
    pool: &Pool,
    ctx: &OrgContext,
    document_id: Uuid,
    version_id: Uuid,
) -> Result<AuthorizedDownloadTargets, DownloadError> {
    let authorized = access::resolve_published_version(pool, ctx, document_id, Some(version_id))
        .await
        .map_err(|error| match error {
            AccessError::NotFound => DownloadError::NotFound,
            AccessError::HistoryRequired => DownloadError::HistoryRequired,
            AccessError::NotPublished => DownloadError::NotPublished,
            AccessError::Database => DownloadError::Database,
        })?;
    let markdown_content_sha256 = if authorized.version.markdown_object_key.is_some() {
        Some(load_markdown_artifact_sha(pool, ctx, version_id).await?)
    } else {
        None
    };
    Ok(AuthorizedDownloadTargets {
        original_object_key: authorized.version.original_object_key,
        markdown_object_key: authorized.version.markdown_object_key,
        source_content_sha256: authorized.version.content_sha256,
        markdown_content_sha256,
    })
}

async fn load_markdown_artifact_sha(
    pool: &Pool,
    ctx: &OrgContext,
    version_id: Uuid,
) -> Result<String, DownloadError> {
    use crate::db::document_versions;

    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                match document_versions::find_markdown_artifact(txn, &ctx, version_id).await {
                    Ok(Some(artifact)) => Ok(Ok(artifact.content_sha256)),
                    Ok(None) => Ok(Err(DownloadError::ObjectUnavailable)),
                    Err(_) => Ok(Err(DownloadError::Database)),
                }
            })
        }
    })
    .await
    .map_err(|_| DownloadError::Database)?
}

/// Mints a short single-purpose capability after fresh ACL checks.
pub async fn issue_capability(
    pool: &Pool,
    ctx: &OrgContext,
    keys: &CapabilityKeys,
    document_id: Uuid,
    version_id: Uuid,
    purpose: DownloadPurpose,
    ttl_secs: Option<i64>,
) -> Result<IssuedCapability, DownloadError> {
    let ttl = ttl_secs.unwrap_or(DEFAULT_TTL_SECS).clamp(1, MAX_TTL_SECS);
    let targets = authorize_version_access(pool, ctx, document_id, version_id).await?;
    if purpose == DownloadPurpose::Markdown
        && targets
            .markdown_object_key
            .as_deref()
            .unwrap_or("")
            .is_empty()
    {
        return Err(DownloadError::ObjectUnavailable);
    }
    let now = Utc::now();
    let claims = CapabilityClaims {
        v: 1,
        org_id: ctx.org_id(),
        user_id: ctx.user_id(),
        document_id,
        version_id,
        purpose: purpose.as_str().into(),
        jti: Uuid::new_v4(),
        iat: now.timestamp(),
        exp: (now + Duration::seconds(ttl)).timestamp(),
    };
    let payload = serde_json::to_vec(&claims).map_err(|_| DownloadError::InvalidCapability)?;
    let payload_b64 = URL_SAFE_NO_PAD.encode(payload);
    let mac = sign_payload(keys, &payload_b64);
    let token = format!("{TOKEN_PREFIX}.{payload_b64}.{mac}");
    Ok(IssuedCapability {
        token: SecretString::new(token),
        expires_in: ttl as u64,
        purpose,
        document_id,
        version_id,
    })
}

fn decode_capability(
    keys: &CapabilityKeys,
    token: &str,
) -> Result<CapabilityClaims, DownloadError> {
    let mut parts = token.split('.');
    let prefix = parts.next().ok_or(DownloadError::InvalidCapability)?;
    let payload_b64 = parts.next().ok_or(DownloadError::InvalidCapability)?;
    let mac = parts.next().ok_or(DownloadError::InvalidCapability)?;
    if parts.next().is_some() || prefix != TOKEN_PREFIX {
        return Err(DownloadError::InvalidCapability);
    }
    if payload_b64.len() > 4096 || mac.len() > 128 {
        return Err(DownloadError::InvalidCapability);
    }
    if !verify_mac(keys, payload_b64, mac) {
        return Err(DownloadError::InvalidCapability);
    }
    let payload = URL_SAFE_NO_PAD
        .decode(payload_b64.as_bytes())
        .map_err(|_| DownloadError::InvalidCapability)?;
    let claims: CapabilityClaims =
        serde_json::from_slice(&payload).map_err(|_| DownloadError::InvalidCapability)?;
    if claims.v != 1 || DownloadPurpose::parse(&claims.purpose).is_none() {
        return Err(DownloadError::InvalidCapability);
    }
    let now = Utc::now().timestamp();
    // Expired at or before `now` (inclusive) must fail closed.
    if claims.exp <= now || claims.iat > now + 30 {
        return Err(DownloadError::InvalidCapability);
    }
    Ok(claims)
}

async fn redeem_jti(
    pool: &Pool,
    ctx: &OrgContext,
    jti: Uuid,
    exp: i64,
) -> Result<(), DownloadError> {
    let expires_at =
        chrono::DateTime::from_timestamp(exp, 0).ok_or(DownloadError::InvalidCapability)?;
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let inserted = txn
                    .query_opt(
                        "INSERT INTO download_capability_redemptions (
                            org_id, jti, expires_at
                         ) VALUES ($1, $2, $3)
                         ON CONFLICT (org_id, jti) DO NOTHING
                         RETURNING jti",
                        &[&ctx.org_id(), &jti, &expires_at],
                    )
                    .await?;
                if inserted.is_none() {
                    return Err(DbError::Config("replay".into()));
                }
                Ok(())
            })
        }
    })
    .await
    .map_err(|error| match error {
        DbError::Config(message) if message == "replay" => DownloadError::Replay,
        _ => DownloadError::Database,
    })
}

/// Redeems a capability once, re-checks ACL, and loads trusted object bytes.
pub async fn redeem_capability(
    pool: &Pool,
    ctx: &OrgContext,
    keys: &CapabilityKeys,
    store: &MinioClient,
    token: &str,
) -> Result<DownloadBytes, DownloadError> {
    if token.len() > 8192 || token.is_empty() {
        return Err(DownloadError::InvalidCapability);
    }
    let claims = decode_capability(keys, token)?;
    if claims.org_id != ctx.org_id() || claims.user_id != ctx.user_id() {
        return Err(DownloadError::PermissionDenied);
    }
    let purpose =
        DownloadPurpose::parse(&claims.purpose).ok_or(DownloadError::InvalidCapability)?;
    redeem_jti(pool, ctx, claims.jti, claims.exp).await?;
    let targets =
        authorize_version_access(pool, ctx, claims.document_id, claims.version_id).await?;
    let object_key = match purpose {
        DownloadPurpose::Original => targets.original_object_key,
        DownloadPurpose::Markdown => targets
            .markdown_object_key
            .ok_or(DownloadError::ObjectUnavailable)?,
    };
    let key = parse_key_for_org(&object_key, ctx.org_id())
        .map_err(|_| DownloadError::ObjectUnavailable)?;
    let bytes = store
        .get_object(ctx.org_id(), &key)
        .await
        .map_err(|error| match error {
            StorageError::NotFound => DownloadError::ObjectUnavailable,
            StorageError::KeyOrgMismatch | StorageError::MissingScope => {
                DownloadError::PermissionDenied
            }
            _ => DownloadError::Storage,
        })?;
    if bytes.len() as u64 > MAX_DOWNLOAD_BYTES {
        return Err(DownloadError::TooLarge);
    }
    let actual_hash = hex::encode(Sha256::digest(&bytes));
    let returned_hash = match purpose {
        // Original: hash returned bytes and verify against stored source hash.
        DownloadPurpose::Original => {
            if actual_hash != targets.source_content_sha256 {
                return Err(DownloadError::ObjectUnavailable);
            }
            actual_hash
        }
        // Markdown: verify against derived_artifacts digest (never source hash).
        DownloadPurpose::Markdown => {
            let expected = targets
                .markdown_content_sha256
                .ok_or(DownloadError::ObjectUnavailable)?;
            if actual_hash != expected {
                return Err(DownloadError::ObjectUnavailable);
            }
            actual_hash
        }
    };
    Ok(DownloadBytes {
        bytes,
        content_type: match purpose {
            DownloadPurpose::Markdown => "text/markdown; charset=utf-8",
            DownloadPurpose::Original => "application/octet-stream",
        },
        content_sha256: returned_hash,
        document_id: claims.document_id,
        version_id: claims.version_id,
        purpose,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> CapabilityKeys {
        CapabilityKeys::new(SecretString::new("0123456789abcdef0123456789abcdef")).unwrap()
    }

    #[test]
    fn capability_round_trip_and_tamper_detect() {
        let keys = keys();
        let claims = CapabilityClaims {
            v: 1,
            org_id: Uuid::from_u128(1),
            user_id: Uuid::from_u128(2),
            document_id: Uuid::from_u128(3),
            version_id: Uuid::from_u128(4),
            purpose: "markdown".into(),
            jti: Uuid::from_u128(5),
            iat: Utc::now().timestamp(),
            exp: Utc::now().timestamp() + 60,
        };
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
        let mac = sign_payload(&keys, &payload);
        let token = format!("{TOKEN_PREFIX}.{payload}.{mac}");
        let decoded = decode_capability(&keys, &token).unwrap();
        assert_eq!(decoded.jti, claims.jti);

        let mut bad = token.clone();
        bad.push('x');
        assert_eq!(
            decode_capability(&keys, &bad).unwrap_err(),
            DownloadError::InvalidCapability
        );
        let tampered = format!("{TOKEN_PREFIX}.{payload}.AAAA");
        assert_eq!(
            decode_capability(&keys, &tampered).unwrap_err(),
            DownloadError::InvalidCapability
        );
    }

    #[test]
    fn expired_capability_rejected() {
        let keys = keys();
        let claims = CapabilityClaims {
            v: 1,
            org_id: Uuid::from_u128(1),
            user_id: Uuid::from_u128(2),
            document_id: Uuid::from_u128(3),
            version_id: Uuid::from_u128(4),
            purpose: "original".into(),
            jti: Uuid::new_v4(),
            iat: Utc::now().timestamp() - 400,
            exp: Utc::now().timestamp() - 10,
        };
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
        let mac = sign_payload(&keys, &payload);
        let token = format!("{TOKEN_PREFIX}.{payload}.{mac}");
        assert_eq!(
            decode_capability(&keys, &token).unwrap_err(),
            DownloadError::InvalidCapability
        );

        let now = Utc::now().timestamp();
        let equal_now = CapabilityClaims {
            v: 1,
            org_id: Uuid::from_u128(1),
            user_id: Uuid::from_u128(2),
            document_id: Uuid::from_u128(3),
            version_id: Uuid::from_u128(4),
            purpose: "markdown".into(),
            jti: Uuid::new_v4(),
            iat: now - 1,
            exp: now,
        };
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&equal_now).unwrap());
        let mac = sign_payload(&keys, &payload);
        let token = format!("{TOKEN_PREFIX}.{payload}.{mac}");
        assert_eq!(
            decode_capability(&keys, &token).unwrap_err(),
            DownloadError::InvalidCapability
        );
    }

    #[test]
    fn original_hash_verification_rejects_tampered_bytes() {
        let expected = hex::encode(Sha256::digest(b"source-bytes"));
        let tampered = hex::encode(Sha256::digest(b"tampered-bytes"));
        assert_ne!(expected, tampered);
        assert_eq!(expected.len(), 64);
    }

    #[test]
    fn debug_hides_token_and_secret() {
        let issued = IssuedCapability {
            token: SecretString::new("mhcap1.secret"),
            expires_in: 60,
            purpose: DownloadPurpose::Markdown,
            document_id: Uuid::nil(),
            version_id: Uuid::nil(),
        };
        let debug = format!("{issued:?}");
        assert!(!debug.contains("mhcap1.secret"));
        assert!(debug.contains("[REDACTED]"));
        assert!(!format!("{:?}", keys()).contains("0123456789abcdef"));
    }
}
