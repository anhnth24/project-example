//! Short-lived, single-purpose download capabilities (P1B-R02 / ADR 0007).
//!
//! Capabilities are HMAC-bound to org/user/document/version/purpose/hash/type/size
//! and recorded in PostgreSQL for expiry + replay protection. Original downloads
//! resolve authoritative upload metadata via the reconciliation parent-source model
//! (promoted versions store Markdown hash/size on the published row).
//!
//! Mint/consume expiry uses PostgreSQL `clock_timestamp()`. Redeem fetches under a
//! process-wide byte budget **before** atomic consume so Busy/storage/integrity
//! leave the token retryable. The response body owns the budget permit until the
//! HTTP body completes, is cancelled, or is dropped.

use std::fmt;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use base64::Engine;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use deadpool_postgres::Pool;
use http_body::{Body, Frame};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

use crate::auth::permissions::{resolve_org_context_in_txn, ResolveError};
use crate::config::SecretString;
use crate::db::document_versions;
use crate::db::download_capabilities::{
    self, AuthorizedConsumeOutcome, CapabilityLiveness, DownloadCapabilityRow, DownloadPurpose,
    NewDownloadCapability,
};
use crate::db::error::DbError;
use crate::db::models::DocumentVersion;
use crate::db::pool::with_org_txn;
use crate::db::search::{self, AuthorizedVersionRow};
use crate::services::preview::{trusted_markdown_artifact_from_version, PreviewError};
use crate::services::reconciliation::authoritative_original_source;
use crate::services::retrieval::{PERMISSION_QA_HISTORY, PERMISSION_QA_QUERY};
use crate::storage::blob::{canonicalize_content_type, BlobStore, ObjectExpectation};
use crate::storage::keys::{authorize_key_for_version, parse_key_for_org, ObjectNamespace};
use crate::storage::StorageError;

const CAPABILITY_DOMAIN: &[u8] = b"markhand-download-cap-v1";
const TOKEN_PREFIX: &str = "mhdl1";
/// Default short TTL for a single-purpose download capability.
pub const DEFAULT_CAPABILITY_TTL: Duration = Duration::from_secs(60);
/// Hard upper bound so clients cannot mint long-lived download grants.
pub const MAX_CAPABILITY_TTL: Duration = Duration::from_secs(300);
/// Absolute download stream bound (defense in depth vs upload policy).
pub const DOWNLOAD_MAX_BYTES: u64 = 200 * 1024 * 1024;
/// Process-wide in-flight download byte budget (slightly above one max object).
pub const DEFAULT_DOWNLOAD_BUDGET_BYTES: u64 = 256 * 1024 * 1024;
/// Max concurrent bounded download fetches per process.
pub const DEFAULT_DOWNLOAD_CONCURRENCY: usize = 4;

/// HMAC signer for download capability tokens (secrets never appear in Debug).
#[derive(Clone)]
pub struct CapabilitySigner {
    key: SecretString,
}

impl fmt::Debug for CapabilitySigner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CapabilitySigner")
            .field("key", &"[REDACTED]")
            .finish()
    }
}

/// Opaque capability token — Debug never prints the raw secret material.
#[derive(Clone, PartialEq, Eq)]
pub struct CapabilityToken(SecretString);

impl CapabilityToken {
    pub fn new(value: impl Into<String>) -> Self {
        Self(SecretString::new(value.into()))
    }

    pub fn expose(&self) -> &str {
        self.0.expose()
    }
}

impl fmt::Debug for CapabilityToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CapabilityToken([REDACTED])")
    }
}

impl CapabilitySigner {
    pub fn new(key: SecretString) -> Result<Self, DownloadError> {
        if key.expose().as_bytes().len() < 32 {
            return Err(DownloadError::SignerNotConfigured);
        }
        Ok(Self { key })
    }

    pub fn from_auth_signing_key(key: Option<&SecretString>) -> Result<Self, DownloadError> {
        let key = key.ok_or(DownloadError::SignerNotConfigured)?;
        Self::new(key.clone())
    }

    fn sign_bytes(&self, message: &[u8]) -> [u8; 32] {
        hmac_sha256(self.key.expose().as_bytes(), message)
    }

    pub fn sign_capability(&self, row: &DownloadCapabilityRow) -> CapabilityToken {
        let mac = self.sign_bytes(&canonical_message(row));
        let mac_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac);
        CapabilityToken::new(format!("{TOKEN_PREFIX}.{}.{mac_b64}", row.id))
    }

    pub fn verify_token(
        &self,
        token: &CapabilityToken,
        row: &DownloadCapabilityRow,
    ) -> Result<(), DownloadError> {
        let expected = self.sign_capability(row);
        if !constant_time_eq(token.expose().as_bytes(), expected.expose().as_bytes()) {
            return Err(DownloadError::InvalidToken);
        }
        Ok(())
    }
}

/// Invalid [`DownloadFetchBudget`] configuration (never panics in constructors).
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DownloadBudgetConfigError {
    #[error("download budget max bytes must be non-zero and <= download max")]
    InvalidMaxBytes,
    #[error("download budget concurrency must be in 1..=Semaphore::MAX_PERMITS")]
    InvalidConcurrency,
}

/// Process-wide limiter so concurrent redeems cannot allocate unbounded RAM.
#[derive(Debug)]
pub struct DownloadFetchBudget {
    max_in_flight_bytes: u64,
    in_flight: AtomicU64,
    slots: Arc<Semaphore>,
}

/// RAII permit that releases concurrency slot + reserved bytes on drop.
/// Not `Clone` — cloning would bypass the budget.
pub struct DownloadFetchPermit {
    budget: Arc<DownloadFetchBudget>,
    reserved: u64,
    _slot: OwnedSemaphorePermit,
}

impl fmt::Debug for DownloadFetchPermit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DownloadFetchPermit")
            .field("reserved", &self.reserved)
            .finish_non_exhaustive()
    }
}

impl Drop for DownloadFetchPermit {
    fn drop(&mut self) {
        self.budget
            .in_flight
            .fetch_sub(self.reserved, Ordering::AcqRel);
    }
}

impl DownloadFetchBudget {
    /// Validated constructor. Rejects zero / oversized byte budgets and
    /// concurrency outside `1..=Semaphore::MAX_PERMITS` (Tokio panics above that).
    pub fn try_new(
        max_in_flight_bytes: u64,
        max_concurrent: usize,
    ) -> Result<Arc<Self>, DownloadBudgetConfigError> {
        // Reject 0 and values that cannot reserve even one max object usefully,
        // plus absurd u64::MAX-scale configs that break checked arithmetic headroom.
        if max_in_flight_bytes == 0 || max_in_flight_bytes == u64::MAX {
            return Err(DownloadBudgetConfigError::InvalidMaxBytes);
        }
        if max_concurrent == 0 || max_concurrent > Semaphore::MAX_PERMITS {
            return Err(DownloadBudgetConfigError::InvalidConcurrency);
        }
        Ok(Arc::new(Self {
            max_in_flight_bytes,
            in_flight: AtomicU64::new(0),
            slots: Arc::new(Semaphore::new(max_concurrent)),
        }))
    }

    pub fn default_production() -> Arc<Self> {
        Self::try_new(DEFAULT_DOWNLOAD_BUDGET_BYTES, DEFAULT_DOWNLOAD_CONCURRENCY)
            .expect("compiled-in production download budget is valid")
    }

    /// Generous limits for hermetic service tests (still exercises acquire/release).
    pub fn for_tests() -> Arc<Self> {
        Self::try_new(DEFAULT_DOWNLOAD_BUDGET_BYTES.saturating_mul(4), 32)
            .expect("compiled-in test download budget is valid")
    }

    pub async fn acquire(
        self: &Arc<Self>,
        bytes: u64,
    ) -> Result<DownloadFetchPermit, DownloadError> {
        if bytes == 0 || bytes > DOWNLOAD_MAX_BYTES {
            return Err(DownloadError::TooLarge);
        }
        // Fail-fast: never park the request waiting for a slot (Busy must be retryable).
        let slot = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| DownloadError::Busy)?;
        loop {
            let current = self.in_flight.load(Ordering::Acquire);
            let Some(next) = current.checked_add(bytes) else {
                drop(slot);
                return Err(DownloadError::Busy);
            };
            if next > self.max_in_flight_bytes {
                drop(slot);
                return Err(DownloadError::Busy);
            }
            match self.in_flight.compare_exchange(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Ok(DownloadFetchPermit {
                        budget: Arc::clone(self),
                        reserved: bytes,
                        _slot: slot,
                    });
                }
                Err(_) => continue,
            }
        }
    }

    pub fn in_flight_bytes(&self) -> u64 {
        self.in_flight.load(Ordering::Acquire)
    }
}

/// HTTP body that owns a [`DownloadFetchPermit`] until fully polled, cancelled, or dropped.
/// Not `Clone` — cloning would release the permit early while bytes still stream.
pub struct BudgetedDownloadBody {
    full: Bytes,
    offset: usize,
    chunk_size: usize,
    permit: Option<DownloadFetchPermit>,
}

impl BudgetedDownloadBody {
    pub fn new(bytes: Bytes, permit: DownloadFetchPermit) -> Self {
        Self {
            full: bytes,
            offset: 0,
            chunk_size: usize::MAX,
            permit: Some(permit),
        }
    }

    /// Test helper: emit the body in small frames so callers can observe held budget.
    pub fn with_chunk_size(mut self, chunk_size: usize) -> Self {
        self.chunk_size = chunk_size.max(1);
        self
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.full
    }

    pub fn len(&self) -> usize {
        self.full.len()
    }

    pub fn is_empty(&self) -> bool {
        self.full.is_empty()
    }

    pub fn permit_held(&self) -> bool {
        self.permit.is_some()
    }
}

impl fmt::Debug for BudgetedDownloadBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BudgetedDownloadBody")
            .field("len", &self.full.len())
            .field("offset", &self.offset)
            .field("permit_held", &self.permit.is_some())
            .finish()
    }
}

impl Body for BudgetedDownloadBody {
    type Data = Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        if this.offset >= this.full.len() {
            // Completed stream: release budget immediately (Drop is a backstop).
            this.permit.take();
            return Poll::Ready(None);
        }
        let end = this
            .offset
            .saturating_add(this.chunk_size)
            .min(this.full.len());
        let chunk = this.full.slice(this.offset..end);
        this.offset = end;
        Poll::Ready(Some(Ok(Frame::data(chunk))))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintedDownloadCapability {
    pub capability_id: Uuid,
    pub token: CapabilityToken,
    pub purpose: DownloadPurpose,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub content_sha256: String,
    pub content_type: String,
    pub byte_size: u64,
    pub expires_at: DateTime<Utc>,
}

/// Redeemed download artifact. Body owns the budget permit (not `Clone`).
pub struct DownloadArtifact {
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub purpose: DownloadPurpose,
    pub content_sha256: String,
    pub content_type: String,
    pub filename: Option<String>,
    /// Zero-copy body; owns [`DownloadFetchPermit`] until stream end/cancel/drop.
    pub body: BudgetedDownloadBody,
}

impl fmt::Debug for DownloadArtifact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DownloadArtifact")
            .field("document_id", &self.document_id)
            .field("version_id", &self.version_id)
            .field("purpose", &self.purpose)
            .field("content_sha256", &self.content_sha256)
            .field("content_type", &self.content_type)
            .field("filename", &self.filename)
            .field("body", &self.body)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginalArtifactMeta {
    pub object_key: String,
    pub content_sha256: String,
    pub content_type: String,
    pub byte_size: u64,
    pub source_filename: Option<String>,
    pub source_version_id: Uuid,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DownloadError {
    #[error("permission denied")]
    PermissionDenied,
    #[error("download source not found")]
    NotFound,
    #[error("download capability expired")]
    Expired,
    #[error("download capability already used")]
    Replay,
    #[error("download capability token is invalid")]
    InvalidToken,
    #[error("download capability signer is not configured")]
    SignerNotConfigured,
    #[error("invalid download purpose")]
    InvalidPurpose,
    #[error("invalid capability ttl")]
    InvalidTtl,
    #[error("storage unavailable")]
    StorageUnavailable,
    #[error("storage error")]
    Storage,
    #[error("integrity check failed")]
    Integrity,
    #[error("object exceeds download size bound")]
    TooLarge,
    #[error("download capacity busy")]
    Busy,
    #[error("database error")]
    Database,
}

impl DownloadError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::PermissionDenied => "download_permission_denied",
            Self::NotFound => "download_not_found",
            Self::Expired => "download_expired",
            Self::Replay => "download_replay",
            Self::InvalidToken => "download_invalid_token",
            Self::SignerNotConfigured => "download_signer_unavailable",
            Self::InvalidPurpose => "download_invalid_purpose",
            Self::InvalidTtl => "download_invalid_ttl",
            Self::StorageUnavailable => "download_storage_unavailable",
            Self::Storage => "download_storage",
            Self::Integrity => "download_integrity",
            Self::TooLarge => "download_too_large",
            Self::Busy => "download_busy",
            Self::Database => "download_database",
        }
    }
}

impl From<ResolveError> for DownloadError {
    fn from(_: ResolveError) -> Self {
        Self::PermissionDenied
    }
}

impl From<DbError> for DownloadError {
    fn from(_: DbError) -> Self {
        Self::Database
    }
}

impl From<StorageError> for DownloadError {
    fn from(value: StorageError) -> Self {
        match value {
            StorageError::NotFound => Self::NotFound,
            StorageError::KeyOrgMismatch
            | StorageError::MissingScope
            | StorageError::InvalidKey => Self::PermissionDenied,
            StorageError::ObjectTooLarge => Self::TooLarge,
            StorageError::PreconditionFailed => Self::Integrity,
            _ => Self::Storage,
        }
    }
}

impl From<PreviewError> for DownloadError {
    fn from(value: PreviewError) -> Self {
        match value {
            PreviewError::PermissionDenied => Self::PermissionDenied,
            PreviewError::NotFound | PreviewError::MarkdownMissing => Self::NotFound,
            PreviewError::Integrity | PreviewError::InvalidUtf8 => Self::Integrity,
            PreviewError::TooLarge => Self::TooLarge,
            PreviewError::StorageUnavailable => Self::StorageUnavailable,
            PreviewError::Storage => Self::Storage,
            PreviewError::Database => Self::Database,
        }
    }
}

fn canonical_message(row: &DownloadCapabilityRow) -> Vec<u8> {
    let mut out = Vec::with_capacity(320);
    out.extend_from_slice(CAPABILITY_DOMAIN);
    out.push(b'|');
    out.extend_from_slice(row.id.as_bytes());
    out.push(b'|');
    out.extend_from_slice(row.org_id.as_bytes());
    out.push(b'|');
    out.extend_from_slice(row.user_id.as_bytes());
    out.push(b'|');
    out.extend_from_slice(row.document_id.as_bytes());
    out.push(b'|');
    out.extend_from_slice(row.version_id.as_bytes());
    out.push(b'|');
    out.extend_from_slice(row.purpose.as_str().as_bytes());
    out.push(b'|');
    out.extend_from_slice(row.content_sha256.as_bytes());
    out.push(b'|');
    out.extend_from_slice(row.content_type.as_bytes());
    out.push(b'|');
    out.extend_from_slice(row.byte_size.to_string().as_bytes());
    out.push(b'|');
    out.extend_from_slice(row.expires_at.timestamp().to_string().as_bytes());
    out
}

fn parse_capability_token(raw: &str) -> Result<(Uuid, CapabilityToken), DownloadError> {
    let mut parts = raw.split('.');
    let prefix = parts.next().ok_or(DownloadError::InvalidToken)?;
    let id = parts.next().ok_or(DownloadError::InvalidToken)?;
    let mac = parts.next().ok_or(DownloadError::InvalidToken)?;
    if parts.next().is_some() || prefix != TOKEN_PREFIX || mac.is_empty() {
        return Err(DownloadError::InvalidToken);
    }
    let id = Uuid::parse_str(id).map_err(|_| DownloadError::InvalidToken)?;
    Ok((id, CapabilityToken::new(raw.to_string())))
}

fn normalize_stored_content_type(raw: &str) -> Result<String, DownloadError> {
    canonicalize_content_type(raw).ok_or(DownloadError::Integrity)
}

fn liveness_error(live: CapabilityLiveness) -> Result<(), DownloadError> {
    match live {
        CapabilityLiveness::Open => Ok(()),
        CapabilityLiveness::Expired => Err(DownloadError::Expired),
        CapabilityLiveness::Replay => Err(DownloadError::Replay),
        CapabilityLiveness::NotFound => Err(DownloadError::NotFound),
    }
}

/// Resolve original upload identity for a published version (reconciliation model).
pub fn resolve_original_artifact_meta(
    versions: &[DocumentVersion],
    published: &AuthorizedVersionRow,
) -> Result<OriginalArtifactMeta, DownloadError> {
    let source = authoritative_original_source(versions, &published.original_object_key)
        .ok_or(DownloadError::NotFound)?;
    let byte_size = source
        .byte_size
        .ok_or(DownloadError::NotFound)
        .and_then(|value| u64::try_from(value).map_err(|_| DownloadError::Integrity))?;
    if byte_size == 0 || byte_size > DOWNLOAD_MAX_BYTES {
        return Err(DownloadError::TooLarge);
    }
    let content_type = normalize_stored_content_type(
        source
            .source_content_type
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("application/octet-stream"),
    )?;
    Ok(OriginalArtifactMeta {
        object_key: source.original_object_key.clone(),
        content_sha256: source.content_sha256.clone(),
        content_type,
        byte_size,
        source_filename: source.source_filename.clone(),
        source_version_id: source.id,
    })
}

/// Mints a short-lived capability after fresh authorization. Never returns object keys.
pub async fn mint_download_capability(
    pool: &Pool,
    signer: &CapabilitySigner,
    org_id: Uuid,
    user_id: Uuid,
    document_id: Uuid,
    version_id: Uuid,
    purpose: DownloadPurpose,
    ttl: Duration,
) -> Result<MintedDownloadCapability, DownloadError> {
    if ttl.is_zero() || ttl > MAX_CAPABILITY_TTL {
        return Err(DownloadError::InvalidTtl);
    }
    let ttl_secs = i64::try_from(ttl.as_secs()).map_err(|_| DownloadError::InvalidTtl)?;
    if ttl_secs <= 0 {
        return Err(DownloadError::InvalidTtl);
    }
    let ctx = resolve_org_context_in_txn(pool, org_id, user_id).await?;
    if !ctx.has_permission(PERMISSION_QA_QUERY) {
        return Err(DownloadError::PermissionDenied);
    }
    if ctx.allowed_collection_ids().is_empty() {
        return Err(DownloadError::PermissionDenied);
    }

    let (row, versions) = with_org_txn(pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let row =
                    search::load_authorized_version_for_read(txn, &ctx, document_id, version_id)
                        .await?;
                let versions = document_versions::list_by_document(txn, &ctx, document_id).await?;
                Ok((row, versions))
            })
        }
    })
    .await?;
    let row = row.ok_or(DownloadError::NotFound)?;

    if row.org_id != org_id || !ctx.allows_collection(row.collection_id) {
        return Err(DownloadError::PermissionDenied);
    }
    if !row.is_current && !ctx.has_permission(PERMISSION_QA_HISTORY) {
        return Err(DownloadError::PermissionDenied);
    }

    let (content_sha256, content_type, byte_size) = match purpose {
        DownloadPurpose::Original => {
            let original = resolve_original_artifact_meta(&versions, &row)?;
            (
                original.content_sha256,
                original.content_type,
                i64::try_from(original.byte_size).map_err(|_| DownloadError::Integrity)?,
            )
        }
        DownloadPurpose::Markdown => {
            let artifact = trusted_markdown_artifact_from_version(&row)?;
            (
                artifact.content_sha256,
                normalize_stored_content_type(&artifact.content_type)?,
                i64::try_from(artifact.byte_size).map_err(|_| DownloadError::Integrity)?,
            )
        }
    };

    let capability_id = Uuid::new_v4();
    let stored = with_org_txn(pool, &ctx, {
        let ctx = ctx.clone();
        let content_sha256 = content_sha256.clone();
        let content_type = content_type.clone();
        move |txn| {
            Box::pin(async move {
                download_capabilities::insert(
                    txn,
                    &ctx,
                    NewDownloadCapability {
                        id: capability_id,
                        document_id,
                        version_id,
                        purpose,
                        content_sha256: &content_sha256,
                        content_type: &content_type,
                        byte_size,
                        ttl_secs,
                    },
                )
                .await
            })
        }
    })
    .await?;

    let token = signer.sign_capability(&stored);
    Ok(MintedDownloadCapability {
        capability_id: stored.id,
        token,
        purpose,
        document_id,
        version_id,
        content_sha256: stored.content_sha256,
        content_type: stored.content_type,
        byte_size: u64::try_from(stored.byte_size).unwrap_or(0),
        expires_at: stored.expires_at,
    })
}

async fn authorize_download_source(
    pool: &Pool,
    org_id: Uuid,
    user_id: Uuid,
    document_id: Uuid,
    version_id: Uuid,
    purpose: DownloadPurpose,
    bound_sha: &str,
    bound_type: &str,
    bound_size: i64,
) -> Result<(String, String, String, u64, Option<String>), DownloadError> {
    let ctx = resolve_org_context_in_txn(pool, org_id, user_id).await?;
    if !ctx.has_permission(PERMISSION_QA_QUERY) {
        return Err(DownloadError::PermissionDenied);
    }
    let (version, versions) = with_org_txn(pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let version =
                    search::load_authorized_version_for_read(txn, &ctx, document_id, version_id)
                        .await?;
                let versions = document_versions::list_by_document(txn, &ctx, document_id).await?;
                Ok((version, versions))
            })
        }
    })
    .await?;
    let version = version.ok_or(DownloadError::PermissionDenied)?;
    if !version.is_current && !ctx.has_permission(PERMISSION_QA_HISTORY) {
        return Err(DownloadError::PermissionDenied);
    }
    if !ctx.allows_collection(version.collection_id) {
        return Err(DownloadError::PermissionDenied);
    }

    let (object_key_raw, expected_sha, content_type, byte_size, filename) = match purpose {
        DownloadPurpose::Original => {
            let original = resolve_original_artifact_meta(&versions, &version)?;
            (
                original.object_key,
                original.content_sha256,
                original.content_type,
                original.byte_size,
                original.source_filename,
            )
        }
        DownloadPurpose::Markdown => {
            let artifact = trusted_markdown_artifact_from_version(&version)?;
            (
                artifact.object_key,
                artifact.content_sha256,
                normalize_stored_content_type(&artifact.content_type)?,
                artifact.byte_size,
                Some("preview.md".into()),
            )
        }
    };

    if expected_sha != bound_sha
        || content_type != bound_type
        || i64::try_from(byte_size).ok() != Some(bound_size)
    {
        return Err(DownloadError::Integrity);
    }
    if byte_size > DOWNLOAD_MAX_BYTES {
        return Err(DownloadError::TooLarge);
    }

    let key = parse_key_for_org(&object_key_raw, org_id)?;
    if purpose == DownloadPurpose::Markdown && key.namespace() != ObjectNamespace::Trusted {
        return Err(DownloadError::PermissionDenied);
    }
    authorize_key_for_version(&key, version_id)?;
    Ok((
        object_key_raw,
        expected_sha,
        content_type,
        byte_size,
        filename,
    ))
}

/// Redeems a capability exactly once. Fetch + verify happen **before** consume so
/// Busy/storage/integrity leave the token retryable. Only the consume winner returns a body.
pub async fn redeem_download_capability<S: BlobStore>(
    pool: &Pool,
    storage: &S,
    signer: &CapabilitySigner,
    budget: &Arc<DownloadFetchBudget>,
    org_id: Uuid,
    user_id: Uuid,
    token_raw: &str,
) -> Result<DownloadArtifact, DownloadError> {
    if token_raw.is_empty() || token_raw.len() > 512 {
        return Err(DownloadError::InvalidToken);
    }
    let (capability_id, token) = parse_capability_token(token_raw)?;

    // 1) Fresh auth + load/verify capability (no consume yet).
    let ctx = resolve_org_context_in_txn(pool, org_id, user_id).await?;
    if !ctx.has_permission(PERMISSION_QA_QUERY) {
        return Err(DownloadError::PermissionDenied);
    }
    let capability = with_org_txn(pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(
                async move { download_capabilities::get_by_id(txn, &ctx, capability_id).await },
            )
        }
    })
    .await?
    .ok_or(DownloadError::NotFound)?;

    if capability.user_id != user_id || capability.org_id != org_id {
        return Err(DownloadError::PermissionDenied);
    }
    signer.verify_token(&token, &capability)?;

    // Soft liveness (DB clock) — avoid expensive fetch when already dead.
    let live = with_org_txn(pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                download_capabilities::classify_liveness(txn, &ctx, capability_id).await
            })
        }
    })
    .await?;
    liveness_error(live)?;

    // 2) Fresh ACL for the bound document/version.
    let (object_key_raw, _expected_sha, _content_type, byte_size, filename) =
        authorize_download_source(
            pool,
            org_id,
            user_id,
            capability.document_id,
            capability.version_id,
            capability.purpose,
            &capability.content_sha256,
            &capability.content_type,
            capability.byte_size,
        )
        .await?;

    let key = parse_key_for_org(&object_key_raw, org_id)?;

    // 3) Acquire budget, then bounded fetch+verify (token still open on failure).
    let permit = budget.acquire(byte_size).await?;
    let fetched = match storage
        .get_object_bounded(
            org_id,
            &key,
            DOWNLOAD_MAX_BYTES.min(byte_size),
            &ObjectExpectation {
                content_sha256: &capability.content_sha256,
                content_length: byte_size,
                content_type: Some(capability.content_type.as_str()),
            },
        )
        .await
    {
        Ok(fetched) => fetched,
        Err(error) => {
            drop(permit);
            return Err(DownloadError::from(error));
        }
    };

    // 4) Single atomic auth+consume at the DB (FOR UPDATE + conditional UPDATE WHERE EXISTS).
    //    Post-fetch OrgContext re-resolve alone is not sufficient — revoke/delete must be
    //    evaluated in the same statement that sets consumed_at.
    let ctx = match resolve_org_context_in_txn(pool, org_id, user_id).await {
        Ok(ctx) => ctx,
        Err(_) => {
            drop(fetched);
            drop(permit);
            return Err(DownloadError::PermissionDenied);
        }
    };
    let expected_document_id = capability.document_id;
    let expected_version_id = capability.version_id;
    let expected_purpose = capability.purpose;
    let expected_sha = capability.content_sha256.clone();
    let expected_type = capability.content_type.clone();
    let expected_size = capability.byte_size;
    let outcome = with_org_txn(pool, &ctx, {
        let ctx = ctx.clone();
        let expected_sha = expected_sha.clone();
        let expected_type = expected_type.clone();
        move |txn| {
            Box::pin(async move {
                download_capabilities::consume_authorized_or_classify(
                    txn,
                    &ctx,
                    capability_id,
                    expected_document_id,
                    expected_version_id,
                    expected_purpose,
                    &expected_sha,
                    &expected_type,
                    expected_size,
                )
                .await
            })
        }
    })
    .await?;

    let consumed = match outcome {
        AuthorizedConsumeOutcome::Consumed(row) => row,
        AuthorizedConsumeOutcome::Expired => {
            drop(fetched);
            drop(permit);
            return Err(DownloadError::Expired);
        }
        AuthorizedConsumeOutcome::Replay => {
            // Concurrent loser: drop bytes + permit; winner alone returns a body.
            drop(fetched);
            drop(permit);
            return Err(DownloadError::Replay);
        }
        AuthorizedConsumeOutcome::PermissionDenied => {
            // Auth failed under the lock — token left open (retryable); no body.
            drop(fetched);
            drop(permit);
            return Err(DownloadError::PermissionDenied);
        }
        AuthorizedConsumeOutcome::NotFound => {
            drop(fetched);
            drop(permit);
            return Err(DownloadError::NotFound);
        }
    };

    if consumed.content_sha256 != capability.content_sha256
        || consumed.content_type != capability.content_type
        || consumed.byte_size != capability.byte_size
    {
        drop(fetched);
        drop(permit);
        return Err(DownloadError::Integrity);
    }

    Ok(DownloadArtifact {
        document_id: consumed.document_id,
        version_id: consumed.version_id,
        purpose: consumed.purpose,
        content_sha256: consumed.content_sha256,
        content_type: consumed.content_type,
        filename,
        body: BudgetedDownloadBody::new(fetched.bytes, permit),
    })
}

/// HMAC-SHA256 without adding a direct `hmac` dependency.
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
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
    for i in 0..BLOCK {
        ipad[i] ^= key_block[i];
        opad[i] ^= key_block[i];
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

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (left, right) in a.iter().zip(b.iter()) {
        diff |= left ^ right;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::{DocumentState, PublicationState};
    use chrono::TimeZone;
    use http_body_util::BodyExt;

    fn sample_row() -> DownloadCapabilityRow {
        DownloadCapabilityRow {
            id: Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap(),
            org_id: Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap(),
            user_id: Uuid::parse_str("22222222-2222-2222-2222-222222222201").unwrap(),
            document_id: Uuid::parse_str("66666666-6666-6666-6666-666666666601").unwrap(),
            version_id: Uuid::parse_str("77777777-7777-7777-7777-777777777701").unwrap(),
            purpose: DownloadPurpose::Markdown,
            content_sha256: "ab".repeat(32),
            content_type: "text/markdown; charset=utf-8".into(),
            byte_size: 128,
            expires_at: Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
            consumed_at: None,
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        }
    }

    fn version(
        id: Uuid,
        parent: Option<Uuid>,
        number: i32,
        original_key: &str,
        content_sha256: String,
        markdown_key: Option<&str>,
        content_type: &str,
        byte_size: i64,
    ) -> DocumentVersion {
        DocumentVersion {
            id,
            org_id: Uuid::new_v4(),
            document_id: Uuid::new_v4(),
            version_number: number,
            parent_version_id: parent,
            publication_state: if markdown_key.is_some() {
                PublicationState::Published
            } else {
                PublicationState::Draft
            },
            is_current: markdown_key.is_some(),
            content_sha256,
            original_object_key: original_key.into(),
            markdown_object_key: markdown_key.map(str::to_string),
            source_filename: Some("source.pdf".into()),
            source_content_type: Some(content_type.into()),
            byte_size: Some(byte_size),
            effective_from: Utc::now(),
            effective_to: None,
            change_summary: None,
            created_by_user_id: Uuid::new_v4(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn original_meta_uses_parent_upload_not_promoted_markdown_hash() {
        let source_id = Uuid::new_v4();
        let published_id = Uuid::new_v4();
        let original_key = "quarantine/aa/bb";
        let original_sha = "aa".repeat(32);
        let markdown_sha = "bb".repeat(32);
        let versions = vec![
            version(
                source_id,
                None,
                1,
                original_key,
                original_sha.clone(),
                None,
                "application/pdf",
                2048,
            ),
            version(
                published_id,
                Some(source_id),
                2,
                original_key,
                markdown_sha.clone(),
                Some("trusted/aa/bb/cc"),
                "text/markdown; charset=utf-8",
                100,
            ),
        ];
        let published = AuthorizedVersionRow {
            org_id: versions[1].org_id,
            collection_id: Uuid::new_v4(),
            document_id: versions[1].document_id,
            version_id: published_id,
            version_number: 2,
            parent_version_id: Some(source_id),
            content_sha256: markdown_sha.clone(),
            original_object_key: original_key.into(),
            markdown_object_key: Some("trusted/aa/bb/cc".into()),
            markdown_artifact_key: Some("trusted/aa/bb/cc".into()),
            markdown_artifact_sha256: Some(markdown_sha),
            markdown_artifact_content_type: Some("text/markdown; charset=utf-8".into()),
            markdown_artifact_byte_size: Some(100),
            source_filename: Some("source.pdf".into()),
            source_content_type: Some("text/markdown; charset=utf-8".into()),
            byte_size: Some(100),
            document_state: DocumentState::Indexed,
            deleted_at: None,
            publication_state: PublicationState::Published,
            is_current: true,
            effective_from: Utc::now(),
            effective_to: None,
        };
        let original = resolve_original_artifact_meta(&versions, &published).unwrap();
        assert_eq!(original.content_sha256, original_sha);
        assert_eq!(original.byte_size, 2048);
        assert_eq!(original.content_type, "application/pdf");
        assert_eq!(original.source_version_id, source_id);
    }

    #[test]
    fn capability_token_debug_is_redacted_and_binds_type_size() {
        let signer =
            CapabilitySigner::new(SecretString::new("unit-test-download-signing-key-32b!"))
                .unwrap();
        let row = sample_row();
        let token = signer.sign_capability(&row);
        assert!(format!("{token:?}").contains("REDACTED"));
        assert!(!format!("{token:?}").contains(token.expose()));
        assert!(signer.verify_token(&token, &row).is_ok());

        let mut other = row.clone();
        other.byte_size = 999;
        assert_eq!(
            signer.verify_token(&token, &other),
            Err(DownloadError::InvalidToken)
        );
        other = row.clone();
        other.content_type = "application/pdf".into();
        assert_eq!(
            signer.verify_token(&token, &other),
            Err(DownloadError::InvalidToken)
        );
    }

    #[test]
    fn token_does_not_embed_object_key_or_bucket() {
        let signer =
            CapabilitySigner::new(SecretString::new("unit-test-download-signing-key-32b!"))
                .unwrap();
        let token = signer.sign_capability(&sample_row());
        assert!(!token.expose().contains("trusted/"));
        assert!(!token.expose().contains("minio"));
        assert!(!token.expose().contains("bucket"));
    }

    #[test]
    fn budget_try_new_rejects_invalid_config() {
        assert_eq!(
            DownloadFetchBudget::try_new(0, 1).err(),
            Some(DownloadBudgetConfigError::InvalidMaxBytes)
        );
        assert_eq!(
            DownloadFetchBudget::try_new(1_024, 0).err(),
            Some(DownloadBudgetConfigError::InvalidConcurrency)
        );
        assert_eq!(
            DownloadFetchBudget::try_new(1_024, Semaphore::MAX_PERMITS.saturating_add(1)).err(),
            Some(DownloadBudgetConfigError::InvalidConcurrency)
        );
        assert!(DownloadFetchBudget::try_new(1_024, 2).is_ok());
    }

    #[tokio::test]
    async fn download_budget_rejects_when_bytes_exhausted_checked_add() {
        assert_eq!(
            DownloadFetchBudget::try_new(u64::MAX, 1).err(),
            Some(DownloadBudgetConfigError::InvalidMaxBytes)
        );
        let budget = DownloadFetchBudget::try_new(1_024, 2).unwrap();
        let a = budget.acquire(800).await.unwrap();
        assert_eq!(budget.in_flight_bytes(), 800);
        assert_eq!(budget.acquire(400).await.err(), Some(DownloadError::Busy));

        // Fill exact max so next reservation uses checked_add and exceeds budget.
        let near = DownloadFetchBudget::try_new(DOWNLOAD_MAX_BYTES, 2).unwrap();
        let p1 = near.acquire(DOWNLOAD_MAX_BYTES).await.unwrap();
        assert_eq!(
            near.acquire(1).await.err(),
            Some(DownloadError::Busy),
            "checked_add path must reject when next would exceed budget"
        );

        // Overflow edge: current near u64::MAX - bytes.
        let overflow_budget = DownloadFetchBudget::try_new(u64::MAX - 1, 2).unwrap();
        // Reserve DOWNLOAD_MAX_BYTES, then attempt another that would overflow if using wrapping add.
        let p2 = overflow_budget.acquire(DOWNLOAD_MAX_BYTES).await.unwrap();
        // Manually poison in_flight to Max - 10 to exercise checked_add overflow branch.
        overflow_budget
            .in_flight
            .store(u64::MAX - 10, Ordering::Release);
        assert_eq!(
            overflow_budget.acquire(32).await.err(),
            Some(DownloadError::Busy)
        );
        // Reset before drop so Drop's fetch_sub does not underflow the test counter.
        overflow_budget
            .in_flight
            .store(DOWNLOAD_MAX_BYTES, Ordering::Release);

        drop(a);
        drop(p1);
        drop(p2);
        assert_eq!(budget.in_flight_bytes(), 0);
        let _b = budget.acquire(400).await.unwrap();
    }

    #[tokio::test]
    async fn budgeted_body_holds_permit_until_consumed_or_dropped() {
        let budget = DownloadFetchBudget::try_new(1_024, 2).unwrap();
        let permit = budget.acquire(16).await.unwrap();
        let mut body = BudgetedDownloadBody::new(Bytes::from_static(b"0123456789abcdef"), permit)
            .with_chunk_size(4);
        assert!(body.permit_held());
        assert_eq!(budget.in_flight_bytes(), 16);

        let frame = std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx))
            .await
            .expect("frame present")
            .expect("infallible body");
        assert_eq!(frame.into_data().expect("data frame").as_ref(), b"0123");
        assert!(body.permit_held(), "slow body must keep budget mid-stream");
        assert_eq!(budget.in_flight_bytes(), 16);

        // Cancellation / drop before completion releases budget.
        drop(body);
        assert_eq!(budget.in_flight_bytes(), 0);

        let permit = budget.acquire(16).await.unwrap();
        let body = BudgetedDownloadBody::new(Bytes::from_static(b"0123456789abcdef"), permit)
            .with_chunk_size(8);
        let collected = body.collect().await.unwrap().to_bytes();
        assert_eq!(collected.as_ref(), b"0123456789abcdef");
        assert_eq!(
            budget.in_flight_bytes(),
            0,
            "full consume must release permit"
        );
    }
}
